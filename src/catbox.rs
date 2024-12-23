use crate::config::Catbox;
use anyhow::Result;
use reqwest::multipart;
use tokio::io::AsyncRead;
use tokio_util::io::ReaderStream;

pub struct CatboxUploader {
    api_url: String,
    userhash: Option<String>,
}

impl CatboxUploader {
    /// 构造方法：从配置初始化
    pub fn new(catbox: &Catbox) -> Self {
        Self {
            api_url: catbox.api_url.clone(),
            userhash: catbox.userhash.clone(),
        }
    }

    /// 上传文件到 Catbox
    pub async fn upload(&self, name: &str, file_path: &str) -> Result<String> {
        let client = reqwest::Client::new();

        // 创建 multipart 表单数据
        let form = multipart::Form::new()
            .text("reqtype", "fileupload")
            .text(
                "userhash",
                self.userhash.clone().unwrap_or_else(|| "".to_string()), // 默认空字符串
            )
            .part(
                "fileToUpload",
                multipart::Part::file(file_path)?
                    .mime_str("application/octet-stream")?,
            );

        // 发送 POST 请求到 Catbox API
        let response = client.post(&self.api_url).multipart(form).send().await?;

        // 检查响应状态
        if response.status().is_success() {
            let body = response.text().await?;
            if body.starts_with("https://") {
                Ok(body.trim().to_string()) // 返回文件 URL
            } else {
                Err(anyhow::anyhow!(
                    "Unexpected response from Catbox: {}",
                    body
                ))
            }
        } else {
            Err(anyhow::anyhow!(
                "Failed to upload file: {}",
                response.status()
            ))
        }
    }

    /// 使用异步流上传文件
    pub async fn upload_from_reader<R: AsyncRead + Unpin>(
        &self,
        name: &str,
        reader: &mut R,
    ) -> Result<String> {
        let client = reqwest::Client::new();

        // 将异步流转换为 reqwest 支持的流
        let stream = ReaderStream::new(reader);
        let body = reqwest::Body::wrap_stream(stream);

        // 创建 multipart 表单数据
        let part = multipart::Part::stream(body).file_name(name.to_string());
        let form = multipart::Form::new()
            .text("reqtype", "fileupload")
            .text(
                "userhash",
                self.userhash.clone().unwrap_or_else(|| "".to_string()),
            )
            .part("fileToUpload", part);

        // 发送 POST 请求到 Catbox API
        let response = client.post(&self.api_url).multipart(form).send().await?;

        // 检查响应状态
        if response.status().is_success() {
            let body = response.text().await?;
            if body.starts_with("https://") {
                Ok(body.trim().to_string())
            } else {
                Err(anyhow::anyhow!(
                    "Unexpected response from Catbox: {}",
                    body
                ))
            }
        } else {
            Err(anyhow::anyhow!(
                "Failed to upload file: {}",
                response.status()
            ))
        }
    }
}
