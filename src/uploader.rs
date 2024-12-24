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
    catbox_uploader: CatboxUploader, // Ensure the CatboxUploader is included
}

impl ExloliUploader {
    pub async fn new(
        config: Config,
        ehentai: EhClient,
        bot: Bot,
        trans: EhTagTransDB,
        userhash: &str, // Adding userhash for the CatboxUploader
    ) -> Result<Self> {
        let telegraph = Telegraph::new(&config.telegraph.author_name)
            .author_url(&config.telegraph.author_url)
            .access_token(&config.telegraph.access_token)
            .create()
            .await?;
        
        let catbox_uploader = CatboxUploader::new(userhash); // Initialize CatboxUploader

        Ok(Self {
            ehentai,
            config,
            telegraph,
            bot,
            trans,
            catbox_uploader, // Include the CatboxUploader in the struct
        })
    }

    // 负责上传图片到 Catbox，并同时创建专辑
    pub async fn create_album_and_upload_images(
        &self,
        gallery: &EhGallery,
        file_urls: Vec<String>, // 文件 URLs
    ) -> Result<String> {
        // 使用 gallery 名称和 config.toml 中的 author_name 创建专辑
        let album_name = &gallery.title_jp(); // 使用画廊的标题作为专辑名称
        let description = &self.config.telegraph.author_name; // 使用 config.toml 中的 author_name 作为描述

        // 创建专辑并上传图片到专辑
        let album_short_url = self.catbox_uploader.create_album(album_name, description, file_urls).await?;

        // 如果创建专辑成功，返回专辑短链接
        Ok(album_short_url)
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

        // 编辑专辑，更新专辑的标题、描述和文件
        self.catbox_uploader.edit_album(short, album_name, description, new_file_urls).await?;

        Ok(())
    }

    // 上传画廊图片到 Catbox，并返回图片 URLs
    async fn upload_gallery_images(
        &self,
        gallery: &EhGallery,
    ) -> Result<Vec<String>> {
        let mut uploaded_urls = Vec::new();

        for page in &gallery.pages {
            let image_url = self.ehentai.get_image_url(page).await?; // 获取每个图片的 URL
            let uploaded_url = self.catbox_uploader.upload_file(&image_url).await?; // 上传图片
            uploaded_urls.push(uploaded_url); // 保存上传后的 URL
        }

        Ok(uploaded_urls)
    }

    // 上传画廊并创建/更新专辑
    pub async fn upload_gallery_and_create_album(
        &self,
        gallery: &EhGallery,
    ) -> Result<String> {
        // 上传所有图片
        let uploaded_urls = self.upload_gallery_images(gallery).await?;

        // 创建专辑并上传图片
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
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(30))
            .build()?;
        let uploader = tokio::spawn(
            async move {
                while let Some((page, (fileindex, url))) = rx.recv().await {
                    let filename = format!("{}.{}", page.hash(), url.split('.').last().unwrap_or("jpg"));
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
}
