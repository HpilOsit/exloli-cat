use std::env;

use anyhow::Result;
use exloli_cat::bot::start_dispatcher;
use exloli_cat::config::{Config, CHANNEL_ID};
use exloli_cat::ehentai::EhClient;
use exloli_cat::tags::EhTagTransDB;
use exloli_cat::uploader::ExloliUploader;
use teloxide::prelude::*;
use teloxide::types::ParseMode;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::new("./config.toml")?;
    CHANNEL_ID.set(config.telegram.channel_id.to_string()).unwrap();

    // NOTE: 全局数据库连接需要用这个变量初始化
    env::set_var("DATABASE_URL", &config.database_url);
    env::set_var("RUST_LOG", &config.log_level);

    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .unwrap();

    // 初始化需要的客户端
    let trans = EhTagTransDB::new(&config.exhentai.trans_file);
    let ehentai = EhClient::new(&config.exhentai.cookie).await?;
    let bot = Bot::new(&config.telegram.token)
        .throttle(Default::default())
        .parse_mode(ParseMode::Html)
        .cache_me();

    // 获取并传递 userhash 配置
    let userhash = config.catbox.userhash.clone(); // 从配置文件中获取 userhash

    // 创建 ExloliUploader，并传递 userhash
    let uploader = ExloliUploader::new(config.clone(), ehentai.clone(), bot.clone(), trans.clone(), userhash.clone()).await?;

    // 启动任务
    let t1 = {
        let uploader = uploader.clone();
        tokio::spawn(async move { uploader.start().await })
    };

    let t2 = {
        let trans = trans.clone();
        tokio::spawn(async move { start_dispatcher(config, uploader, bot, trans).await })
    };

    let t3 = tokio::spawn(async move { trans.start().await });

    // 等待所有异步任务
    tokio::try_join!(t1, t2, t3)?;

    Ok(())
}
