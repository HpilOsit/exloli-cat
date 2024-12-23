use std::time::Duration;
use anyhow::{bail, Result};
use chrono::{Datelike, Utc};
use futures::StreamExt;
use regex::Regex;
use reqwest::{Client, StatusCode};
use telegraph_rs::{html_to_node, Telegraph};
use teloxide::types::MessageId;
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{debug, error, info, Instrument};

use crate::bot::Bot;
use crate::config::Config;
use crate::database::{
    GalleryEntity, ImageEntity, MessageEntity, PageEntity, TelegraphEntity,
};
use crate::ehentai::{EhClient, EhGallery, EhGalleryUrl, GalleryInfo};
use crate::catbox::CatboxUploader;
use crate::tags::EhTagTransDB;

#[derive(Debug, Clone)]
pub struct ExloliUploader {
    ehentai: EhClient,
    telegraph: Telegraph,
    bot: Bot,
    config: Config,
    trans: EhTagTransDB,
}

impl ExloliUploader {
    pub async fn new(
        config: Config,
        ehentai: EhClient,
        bot: Bot,
        trans: EhTagTransDB,
    ) -> Result<Self> {
        let telegraph = Telegraph::new(&config.telegraph.author_name)
            .author_url(&config.telegraph.author_url)
            .access_token(&config.telegraph.access_token)
            .create()
            .await?;
        Ok(Self { ehentai, config, telegraph, bot, trans })
    }

    /// 每隔 interval 分钟检查一次
    pub async fn start(&self) {
        loop {
            info!("开始扫描 E 站本子");
            self.check().await;
            info!("扫描完毕，等待 {:?} 后继续", self.config.interval);
            time::sleep(self.config.interval).await;
        }
    }

    /// 检查画廊并上传/更新
    #[tracing::instrument(skip(self))]
    async fn check(&self) {
        let stream = self
            .ehentai
            .search_iter(&self.config.exhentai.search_params)
            .take(self.config.exhentai.search_count);
        tokio::pin!(stream);
        while let Some(next) = stream.next().await {
            if let Err(err) = self.try_upload(&next, true).await {
                error!("check_and_upload: {:?}", err);
            }
            time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// 上传未上传的画廊
    #[tracing::instrument(skip(self))]
    pub async fn try_upload(&self, gallery: &EhGalleryUrl, check: bool) -> Result<()> {
        if check
            && GalleryEntity::check(gallery.id()).await?
            && MessageEntity::get_by_gallery(gallery.id()).await?.is_some()
        {
            return Ok(());
        }

        let gallery = self.ehentai.get_gallery(gallery).await?;
        self.upload_gallery_image(&gallery).await?;
        let article = self.publish_telegraph_article(&gallery).await?;
        let text = self.create_message_text(&gallery, &article.url).await?;
        let msg = self.bot
            .send_message(self.config.telegram.channel_id.clone(), text)
            .await?;
        MessageEntity::create(msg.id.0, gallery.url.id()).await?;
        TelegraphEntity::create(gallery.url.id(), &article.url).await?;
        GalleryEntity::create(&gallery).await?;

        Ok(())
    }

    /// 上传画廊所有图片到 Catbox
    async fn upload_gallery_image(&self, gallery: &EhGallery) -> Result<()> {
        let mut pages = vec![];
        for page in &gallery.pages {
            match ImageEntity::get_by_hash(page.hash()).await? {
                Some(img) => {
                    PageEntity::create(page.gallery_id(), page.page(), img.id).await?;
                }
                None => pages.push(page.clone()),
            }
        }
        info!("需要上传 {} 张图片", pages.len());

        let concurrent = self.config.threads_num;
        let (tx, mut rx) = tokio::sync::mpsc::channel(concurrent * 2);

        let getter = tokio::spawn(
            async move {
                for page in pages {
                    let rst = self.ehentai.get_image_url(&page).await?;
                    tx.send((page, rst)).await?;
                }
                Result::<()>::Ok(())
            }
            .instrument(tracing::span!(tracing::Level::INFO, "getter")),
        );

        let catbox_uploader = CatboxUploader::new(&self.config.catbox);

        let uploader = tokio::spawn(
            async move {
                while let Some((page, (fileindex, url))) = rx.recv().await {
                    let suffix = url.split('.').last().unwrap_or("jpg");
                    if suffix == "gif" {
                        continue;
                    }
                    let filename = format!("{}.{}", page.hash(), suffix);
                    let bytes = Client::new().get(url).send().await?.bytes().await?;
                    debug!("已下载页面: {}", page.page());

                    let temp_file_path = format!("/tmp/{}", filename);
                    tokio::fs::write(&temp_file_path, &bytes).await?;
                    let uploaded_url = catbox_uploader.upload(&filename, &temp_file_path).await?;
                    tokio::fs::remove_file(&temp_file_path).await?;
                    debug!("已上传到 Catbox: {}", uploaded_url);

                    ImageEntity::create(fileindex, page.hash(), &uploaded_url).await?;
                    PageEntity::create(page.gallery_id(), page.page(), fileindex).await?;
                }
                Result::<()>::Ok(())
            }
            .instrument(tracing::span!(tracing::Level::INFO, "uploader")),
        );

        tokio::try_join!(getter, uploader)?;

        Ok(())
    }

    /// 创建 Telegraph 文章
    async fn publish_telegraph_article<T: GalleryInfo>(
        &self,
        gallery: &T,
    ) -> Result<telegraph_rs::Page> {
        let images = ImageEntity::get_by_gallery_id(gallery.url().id()).await?;
        let mut html = String::new();
        for img in images {
            html.push_str(&format!(r#"<img src="{}">"#, img.url()));
        }
        html.push_str(&format!("<p>图片总数：{}</p>", gallery.pages()));
        let node = html_to_node(&html);
        let title = gallery.title_jp();
        Ok(self.telegraph.create_page(&title, &node, false).await?)
    }

    /// 创建 Telegram 消息正文
    async fn create_message_text<T: GalleryInfo>(
        &self,
        gallery: &T,
        article: &str,
    ) -> Result<String> {
        let re = Regex::new("[-/· ]").unwrap();
        let tags = self.trans.trans_tags(gallery.tags());
        let mut text = String::new();
        for (ns, tag) in tags {
            let tag = tag
                .iter()
                .map(|s| format!("#{}", re.replace_all(s, "_")))
                .collect::<Vec<_>>()
                .join(" ");
            text.push_str(&format!("{}: {}\n", ns, tag));
        }
        text.push_str(&format!("{}: {}\n", "预览", article));
        text.push_str(&format!("{}: {}", "原始地址", gallery.url().url()));
        Ok(text)
    }
}

async fn flatten<T>(handle: JoinHandle<Result<T>>) -> Result<T> {
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err),
        Err(err) => bail!("Task panicked: {:?}", err),
    }
}
