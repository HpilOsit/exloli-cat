use std::backtrace::Backtrace;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use chrono::{Datelike, Utc};
use futures::StreamExt;
use regex::Regex;
use reqwest::{Client, StatusCode};
use telegraph_rs::{html_to_node, Telegraph};
use teloxide::prelude::*;
use teloxide::types::MessageId;
use teloxide::utils::html::{code_inline, link};
use tokio::task::JoinHandle;
use tokio::time;
use tokio::sync::Mutex;
use std::sync::Arc;
use tracing::{debug, error, info, Instrument};

use crate::bot::Bot;
use crate::config::Config;
use crate::database::{
    GalleryEntity, ImageEntity, MessageEntity, PageEntity, PollEntity, TelegraphEntity,
};
use crate::ehentai::{EhClient, EhGallery, EhGalleryUrl, GalleryInfo};
use crate::catbox::CatboxUploader;
use crate::tags::EhTagTransDB;
use crate::utils::pad_left;

#[derive(Debug, Clone)]
pub struct ExloliUploader {
    ehentai: EhClient,
    telegraph: Telegraph,
    bot: Bot,
    config: Config,
    trans: EhTagTransDB,
    catbox_uploader: CatboxUploader,
}

impl ExloliUploader {
    pub async fn new(
        config: Config,
        ehentai: EhClient,
        bot: Bot,
        trans: EhTagTransDB,
        userhash: String, 
    ) -> Result<Self> {
        let telegraph = Telegraph::new(&config.telegraph.author_name)
            .author_url(&config.telegraph.author_url)
            .access_token(&config.telegraph.access_token)
            .create()
            .await?;
            let catbox_uploader = CatboxUploader::new(&userhash);
        Ok(Self { ehentai, config, telegraph, bot, trans, catbox_uploader })
    }
}

impl ExloliUploader {
    // 负责上传图片到 Catbox，并同时创建专辑
    pub async fn create_album_and_upload_images(
        &self,
        gallery: &EhGallery,
        file_urls: Vec<String>, // 文件 URLs
    ) -> Result<String> {
        let album_name = &gallery.title_jp(); // 使用画廊的标题作为专辑名称
        let description = &self.config.telegraph.author_name; // 使用 config.toml 中的 author_name 作为描述

        // 将图片分成若干批次
        let chunk_size = 35;
        let file_chunks = file_urls.chunks(chunk_size);

        let mut all_files = Vec::new();

        // 分批上传文件
        for chunk in file_chunks {
            let file_urls_batch = chunk.to_vec();
            let album_url = self.catbox_uploader.create_album(album_name, description, file_urls_batch).await?;
            all_files.push(album_url);
        }

        // 返回所有生成的专辑链接
        Ok(all_files.join("\\n"))
    }

    // 更新专辑，将新的图片上传并同步更新专辑
    pub async fn update_album_with_new_images(
        &self,
        short: &str,  // 专辑的短链接
        gallery: &EhGallery,
        new_file_urls: Vec<String>, // 新的文件 URLs
    ) -> Result<()> {
        let album_name = &gallery.title_jp(); // 使用画廊的标题作为专辑名称
        let description = &self.config.telegraph.author_name; // 使用 config.toml 中的 author_name 作为描述

        // 将图片分成若干批次
        let chunk_size = 35;
        let file_chunks = new_file_urls.chunks(chunk_size);

        // 分批更新专辑中的文件
        for chunk in file_chunks {
            let new_file_urls_batch = chunk.to_vec();
            self.catbox_uploader.edit_album(short, album_name, description, new_file_urls_batch).await?;
        }

        Ok(())
    }


    async fn upload_gallery_images(
        &self,
        gallery: &EhGallery,
    ) -> Result<Vec<String>> {
        let mut uploaded_urls = Vec::new();

        for page in &gallery.pages {
            let image_url = self.ehentai.get_image_url(page).await?; 
            let uploaded_url = self.catbox_uploader.upload_file(&image_url.1).await?; 
            uploaded_urls.push(uploaded_url); 
        }

        Ok(uploaded_urls)
    }

    // 上传画廊并创建/更新专辑
    pub async fn upload_gallery_and_create_album(
        &self,
        gallery: &EhGallery,
    ) -> Result<String> {

        let uploaded_urls = self.upload_gallery_images(gallery).await?;

        let album_short_url = self.create_album_and_upload_images(gallery, uploaded_urls).await?;

        Ok(album_short_url)
    }

    /// 每隔 interval 分钟检查一次
    pub async fn start(&self) {
        loop {
            info!("开始扫描 E 站 本子");
            self.check().await;
            info!("扫描完毕，等待 {:?} 后继续", self.config.interval);
            time::sleep(self.config.interval).await;
        }
    }

    /// 根据配置文件，扫描前 N 个本子，并进行上传或者更新
    #[tracing::instrument(skip(self))]
    async fn check(&self) {
        let stream = self
            .ehentai
            .search_iter(&self.config.exhentai.search_params)
            .take(self.config.exhentai.search_count);
        tokio::pin!(stream);
        while let Some(next) = stream.next().await {
            if let Err(err) = self.try_update(&next, true).await {
                error!("check_and_update: {:?}\n{}", err, Backtrace::force_capture());
            }
            if let Err(err) = self.try_upload(&next, true).await {
                error!("check_and_upload: {:?}\n{}", err, Backtrace::force_capture());
            }
            time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// 检查指定画廊是否已经上传，如果没有则进行上传
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
        
        let msg = if let Some(parent) = &gallery.parent {
            if let Some(pmsg) = MessageEntity::get_by_gallery(parent.id()).await? {
                self.bot
                    .send_message(self.config.telegram.channel_id.clone(), text)
                    .reply_to_message_id(MessageId(pmsg.id))
                    .await?
            } else {
                self.bot.send_message(self.config.telegram.channel_id.clone(), text).await?
            }
        } else {
            self.bot.send_message(self.config.telegram.channel_id.clone(), text).await?
        };
        
        MessageEntity::create(msg.id.0, gallery.url.id()).await?;
        TelegraphEntity::create(gallery.url.id(), &article.url).await?;
        GalleryEntity::create(&gallery).await?;

        Ok(())
    }

    /// 检查指定画廊是否有更新，比如标题、标签
    #[tracing::instrument(skip(self))]
    pub async fn try_update(&self, gallery: &EhGalleryUrl, check: bool) -> Result<()> {
        let entity = match GalleryEntity::get(gallery.id()).await? {
            Some(v) => v,
            _ => return Ok(()),
        };
        let message = match MessageEntity::get_by_gallery(gallery.id()).await? {
            Some(v) => v,
            _ => return Ok(()),
        };

        let now = Utc::now().date_naive();
        let seed = match now - message.publish_date {
            d if d < chrono::Duration::days(2) => 1,
            d if d < chrono::Duration::days(7) => 3,
            d if d < chrono::Duration::days(14) => 7,
            _ => 14,
        };
        if check && now.day() % seed != 0 {
            return Ok(());
        }

        let gallery = self.ehentai.get_gallery(gallery).await?;

        if gallery.tags != entity.tags.0 || gallery.title != entity.title {
            let telegraph = TelegraphEntity::get(gallery.url.id()).await?.unwrap();
            let text = self.create_message_text(&gallery, &telegraph.url).await?;
            self.bot
                .edit_message_text(
                    self.config.telegram.channel_id.clone(),
                    MessageId(message.id),
                    text,
                )
                .await?;
        }

        GalleryEntity::create(&gallery).await?;

        Ok(())
    }

    /// 重新发布指定画廊的文章，并更新消息
    pub async fn republish(&self, gallery: &GalleryEntity, msg: &MessageEntity) -> Result<()> {
        info!("重新发布：{}", msg.id);
        let article = self.publish_telegraph_article(gallery).await?;
        let text = self.create_message_text(gallery, &article.url).await?;
        self.bot
            .edit_message_text(self.config.telegram.channel_id.clone(), MessageId(msg.id), text)
            .await?;
        TelegraphEntity::update(gallery.id, &article.url).await?;
        Ok(())
    }

    /// 检查 telegraph 文章是否正常
    pub async fn check_telegraph(&self, url: &str) -> Result<bool> {
        Ok(Client::new().head(url).send().await?.status() != StatusCode::NOT_FOUND)
    }
}

impl ExloliUploader {
   /// 获取某个画廊里的所有图片，并且上传到 telegraph，如果已经上传过的，会跳过上传
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
        info!("需要下载&上传 {} 张图片", pages.len());

        let concurrent = self.config.threads_num;
        let (tx, mut rx) = tokio::sync::mpsc::channel(concurrent * 2);
        let client = self.ehentai.clone();

        let getter = tokio::spawn(
            async move {
                for page in pages {
                    let rst = client.get_image_url(&page).await?;
                    info!("已解析：{}", page.page());
                    tx.send((page, rst)).await?;
                }
                Result::<()>::Ok(())
            }
            .in_current_span(),
        );

        let catbox_uploader = CatboxUploader::new(&self.config.catbox.userhash);
        let client = Arc::new(Mutex::new(Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(30))
            .build()?));
        let uploader = tokio::spawn(
            async move {
                while let Some((page, (fileindex, url))) = rx.recv().await {
                    let filename = format!("{}.{}", page.hash(), url.split('.').last().unwrap_or("jpg"));
                    let client = client.clone();  
                    let bytes = client.lock().await.get(url).send().await?.bytes().await?;
                    debug!("已下载: {}", page.page());
                    let uploaded_url = catbox_uploader.upload_file(&filename).await?;
                    debug!("已上传: {}", page.page());
                    ImageEntity::create(fileindex, page.hash(), &uploaded_url).await?;
                    PageEntity::create(page.gallery_id(), page.page(), fileindex).await?;
                }
                Result::<()>::Ok(())
            }
            .in_current_span(),
        );

        tokio::try_join!(flatten(getter), flatten(uploader))?;

        Ok(())
    }

    /// 从数据库中读取某个画廊的所有图片，生成一篇 telegraph 文章
    async fn publish_telegraph_article<T: GalleryInfo>(
        &self,
        gallery: &T,
    ) -> Result<telegraph_rs::Page> {
        let images = ImageEntity::get_by_gallery_id(gallery.url().id()).await?;

        let mut html = String::new();
        if gallery.cover() != 0 && gallery.cover() < images.len() {
            html.push_str(&format!(r#"<img src="{}">"#, images[gallery.cover()].url()))
        }
        for img in images {
            html.push_str(&format!(r#"<img src="{}">"#, img.url()));
        }
        html.push_str(&format!("<p>图片总数：{}</p>", gallery.pages()));

        let node = html_to_node(&html);
        let title = gallery.title_jp();
        Ok(self.telegraph.create_page(&title, &node, false).await?)
    }

    /// 为画廊生成一条可供发送的 telegram 消息正文
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
            text.push_str(&format!("{}: {}\n", code_inline(&pad_left(&ns, 6)), tag))
        }

        text.push_str(
            &format!("{}: {}\n", code_inline("  预览"), link(article, &gallery.title()),),
        );
        text.push_str(&format!("{}: {}", code_inline("原始地址"), gallery.url().url()));

        Ok(text)
    }
}

async fn flatten<T>(handle: JoinHandle<Result<T>>) -> Result<T> {
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err),
        Err(err) => bail!(err),
    }
}

impl ExloliUploader {
    pub async fn reupload(&self, mut galleries: Vec<GalleryEntity>) -> Result<()> {
        if galleries.is_empty() {
            galleries = GalleryEntity::list_scans().await?;
        }
        for gallery in galleries.iter().rev() {
            if let Some(score) = PollEntity::get_by_gallery(gallery.id).await? {
                if score.score > 0.8 {
                    info!("尝试上传画廊：{}", gallery.url());
                    if let Err(err) = self.try_upload(&gallery.url(), true).await {
                        error!("上传失败：{}", err);
                    }
                    time::sleep(Duration::from_secs(60)).await;
                }
            }
        }
        Ok(())
    }

    pub async fn recheck(&self, mut galleries: Vec<GalleryEntity>) -> Result<()> {
        if galleries.is_empty() {
            galleries = GalleryEntity::list_scans().await?;
        }
        for gallery in galleries.iter().rev() {
            let telegraph =
                TelegraphEntity::get(gallery.id).await?.ok_or(anyhow!("找不到 telegraph"))?;
            if let Some(msg) = MessageEntity::get_by_gallery(gallery.id).await? {
                info!("检测画廊：{}", gallery.url());
                if !self.check_telegraph(&telegraph.url).await? {
                    info!("重新上传预览：{}", gallery.url());
                    if let Err(err) = self.republish(gallery, &msg).await {
                        error!("上传失败：{}", err);
                    }
                    time::sleep(Duration::from_secs(60)).await;
                }
            }
            time::sleep(Duration::from_secs(1)).await;
        }
        Ok(())
    }
}
