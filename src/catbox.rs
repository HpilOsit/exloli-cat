use crate::config::Catbox;
use anyhow::Result;
use reqwest::multipart;
use tokio::io::AsyncRead;
use tokio_util::io::ReaderStream;

/// Catbox 上传器结构体
pub struct CatboxUploader {
    api_url: String,
    userhash: Option<String>,
}

impl CatboxUploader {
    /// 构造方法：从配置初始化
    pub fn new(api_url: String, userhash: Option<String>) -> Self {
        Self { api_url, userhash }
    }

    /// 上传文件到 Catbox（直接读取本地路径）
    pub async fn upload(&self, name: &str, file_bytes: &[u8]) -> Result<String> {
        let client = reqwest::Client::new();

        let part = multipart::Part::bytes(file_bytes.to_vec()).file_name(name);
        let form = multipart::Form::new()
            .text("reqtype", "fileupload")
            .text("userhash", self.userhash.clone().unwrap_or_default())
            .part("fileToUpload", part);

        let response = client.post(&self.api_url).multipart(form).send().await?;

        if response.status().is_success() {
            let body = response.text().await?;
            if body.starts_with("https://") {
                Ok(body.trim().to_string())
            } else {
                Err(anyhow::anyhow!("Unexpected response: {}", body))
            }
        } else {
            Err(anyhow::anyhow!(
                "Failed with status code: {}",
                response.status()
            ))
        }
    }

    /// 使用异步流方式上传文件
    pub async fn upload_from_reader<R: AsyncRead + Unpin>(
        &self,
        name: &str,
        reader: &mut R,
    ) -> Result<String> {
        let client = reqwest::Client::new();
        let stream = ReaderStream::new(reader);
        let body = reqwest::Body::wrap_stream(stream);

        let part = multipart::Part::stream(body).file_name(name.to_string());
        let form = multipart::Form::new()
            .text("reqtype", "fileupload")
            .text("userhash", self.userhash.clone().unwrap_or_default())
            .part("fileToUpload", part);

        let response = client.post(&self.api_url).multipart(form).send().await?;

        if response.status().is_success() {
            let body = response.text().await?;
            if body.starts_with("https://") {
                Ok(body.trim().to_string())
            } else {
                Err(anyhow::anyhow!("Unexpected response: {}", body))
            }
        } else {
            Err(anyhow::anyhow!(
                "Upload failed with status code: {}",
                response.status()
            ))
        }
    }
}
