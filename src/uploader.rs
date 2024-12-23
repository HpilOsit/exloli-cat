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
        Ok(Self {
            ehentai,
            config,
            telegraph,
            bot,
            trans,
        })
    }

    /// 每隔 interval 分钟检查一次
    pub async fn start(&self) {
        loop {
            info!("开始扫描 E 站画廊");
            self.check().await;
            info!("扫描完毕，等待 {:?} 后继续", self.config.interval);
            time::sleep(self.config.interval).await;
        }
    }

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

    pub async fn upload_gallery_image(&self, gallery: &EhGallery) -> Result<()> {
        let mut pages = vec![];
        for page in &gallery.pages {
            match ImageEntity::get_by_hash(page.hash()).await? {
                Some(img) => {
                    PageEntity::create(page.gallery_id(), page.page(), img.id).await?;
                }
                None => pages.push(page.clone()),
            }
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel(self.config.threads_num * 2);
        let catbox_uploader = CatboxUploader::new(&self.config.catbox);
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(30))
            .build()?;

        let getter = tokio::spawn(async move {
            for page in pages {
                let rst = client.get(page.url.clone()).send().await?.text().await?;
                tx.send((page, rst)).await?;
            }
            Result::<()>::Ok(())
        });

        let uploader = tokio::spawn(async move {
            while let Some((page, url)) = rx.recv().await {
                let suffix = url.split('.').last().unwrap_or("jpg");
                if suffix == "gif" {
                    continue;
                }

                let filename = format!("{}.{}", page.hash(), suffix);
                let uploaded_url = catbox_uploader.upload(&filename, &page.url).await?;
                debug!("已上传到 Catbox: {}", uploaded_url);
            }
            Result::<()>::Ok(())
        });

        tokio::try_join!(flatten(getter), flatten(uploader))?;

        Ok(())
    }
}

async fn flatten<T>(handle: JoinHandle<Result<T>>) -> Result<T> {
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err),
        Err(err) => bail!("任务崩溃：{:?}", err),
    }
}
