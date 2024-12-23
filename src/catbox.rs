use reqwest::multipart::{Form, Part};
use reqwest::Client;
use anyhow::{Result, anyhow};
use tokio::io::AsyncReadExt;

pub struct CatboxUploader {
    userhash: String,  // Catbox 用户的 userhash
    client: Client,    // HTTP 客户端
}

impl CatboxUploader {
    pub fn new(userhash: &str) -> Self {
        let client = Client::new();
        Self {
            userhash: userhash.to_string(),
            client,
        }
    }

    // 上传文件到 Catbox
    pub async fn upload_file(&self, file_path: &str) -> Result<String> {
        // 读取文件内容
        let mut file = tokio::fs::File::open(file_path).await?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;

        let file_part = Part::stream(buffer);
        let form = Form::new()
            .text("reqtype", "fileupload")
            .text("userhash", self.userhash.clone())
            .part("fileToUpload", file_part);

        // 发起上传请求
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
            Err(anyhow!("Failed to upload file"))
        }
    }

    // 上传 URL 到 Catbox
    pub async fn upload_url(&self, image_url: &str) -> Result<String> {
        let form = Form::new()
            .text("reqtype", "urlupload")
            .text("userhash", self.userhash.clone())
            .text("url", image_url.to_string());

        // 发起上传请求
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
