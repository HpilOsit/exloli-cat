use reqwest::multipart::{Form, Part};
use reqwest::Client;
use anyhow::{Result, anyhow};
use tokio::fs::File;
use std::path::Path;

pub struct CatboxUploader {
    userhash: String,  // Catbox 用户的 userhash
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

    // 上传文件到 Catbox
    pub async fn upload_file(&self, file_path: &str) -> Result<String> {
        // 创建文件部分
        let file = Part::file(file_path)?;
        let form = Form::new()
            .text("reqtype", "fileupload")
            .text("userhash", self.userhash.clone())
            .part("fileToUpload", file);  // 使用 .part() 添加文件部分

        // 发起 POST 请求
        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?;

        // 检查上传是否成功
        if res.status().is_success() {
            let json: serde_json::Value = res.json().await?;
            if let Some(url) = json["fileURL"].as_str() {
                Ok(url.to_string())  // 返回上传成功后的文件 URL
            } else {
                Err(anyhow!("Failed to get file URL from response"))
            }
        } else {
            Err(anyhow!("Failed to upload file"))
        }
    }

    // 上传图片 URL到 Catbox
    pub async fn upload_url(&self, image_url: &str) -> Result<String> {
        let form = Form::new()
            .text("reqtype", "urlupload")
            .text("userhash", self.userhash.clone())
            .text("url", image_url.to_string()); // 使用 to_string() 确保生命周期有效

        // 发起 POST 请求
        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?;

        // 检查上传是否成功
        if res.status().is_success() {
            let json: serde_json::Value = res.json().await?;
            if let Some(url) = json["fileURL"].as_str() {
                Ok(url.to_string())  // 返回上传成功后的文件 URL
            } else {
                Err(anyhow!("Failed to get file URL from response"))
            }
        } else {
            Err(anyhow!("Failed to upload URL"))
        }
    }
}
