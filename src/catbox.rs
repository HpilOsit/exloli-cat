use reqwest::multipart;
use reqwest::Client;
use anyhow::{Result, anyhow};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

pub struct CatboxUploader {
    userhash: String,  // Catbox 用户的 userhash（可以通过注册获得）
    client: Client,    // 用于发起 HTTP 请求的 client
}

impl CatboxUploader {
    pub fn new(userhash: &str) -> Self {
        let client = Client::new();
        Self {
            userhash: userhash.to_string(),
            client,
        }
    }

    pub async fn upload_file(&self, file_path: &str) -> Result<String> {
        // 打开文件
        let file = File::open(file_path).await?;
        let file_name = file_path.split('/').last().unwrap_or("image");
        
        // 创建multipart请求体
        let form = multipart::Form::new()
            .text("reqtype", "fileupload")
            .text("userhash", self.userhash.clone())
            .file("fileToUpload", file_path)?;

        // 向 Catbox API 发起请求
        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?;

        // 解析响应
        if res.status().is_success() {
            let json: serde_json::Value = res.json().await?;
            if let Some(url) = json["fileURL"].as_str() {
                Ok(url.to_string())  // 返回上传成功后的文件URL
            } else {
                Err(anyhow!("Failed to get file URL from response"))
            }
        } else {
            Err(anyhow!("Failed to upload file"))
        }
    }

    pub async fn upload_url(&self, image_url: &str) -> Result<String> {
        let form = multipart::Form::new()
            .text("reqtype", "urlupload")
            .text("userhash", self.userhash.clone())
            .text("url", image_url);

        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?;

        if res.status().is_success() {
            let json: serde_json::Value = res.json().await?;
            if let Some(url) = json["fileURL"].as_str() {
                Ok(url.to_string())
            } else {
                Err(anyhow!("Failed to get file URL from response"))
            }
        } else {
            Err(anyhow!("Failed to upload URL"))
        }
    }
}
