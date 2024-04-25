use std::env;

use futures::executor::block_on;
use once_cell::sync::Lazy;
use sqlx::sqlite::*;
use tracing::info;

pub static DB: Lazy<SqlitePool> = Lazy::new(|| {
    let url = env::var("DATABASE_URL").expect("数据库连接字符串未设置");
    block_on(get_connection_pool(&url))
});

pub async fn get_connection_pool(url: &str) -> SqlitePool {
    info!("初始化数据库连接：{}", url);
    let options = SqliteConnectOptions::new()
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(false)
        .filename(url)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new().connect_with(options).await.expect("数据库连接失败");
    info!("检查数据库迁移");
    sqlx::migrate!("./migrations").run(&pool).await.expect("数据库迁移失败");
    pool
}
