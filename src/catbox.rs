use reqwest::multipart::{Form, Part};
use reqwest::Client;
use anyhow::{Result, anyhow};
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone)]
pub struct CatboxUploader {
    userhash: String, // Catbox 用户的 userhash
    client: Client,   // HTTP 客户端
}

impl CatboxUploader {
    // 构造方法
    pub fn new(userhash: &str) -> Self {
        let client = Client::new(); // 初始化 HTTP 客户端
        Self {
            userhash: userhash.to_string(), // 用户哈希赋值
            client,
        }
    }

    // 上传文件到 Catbox
    pub async fn upload_file(&self, file_path: &str) -> Result<String> {
        let mut file = tokio::fs::File::open(file_path).await?; // 异步打开文件
        let mut buffer = Vec::new(); // 用于存储文件内容
        file.read_to_end(&mut buffer).await?; // 读取文件内容到缓冲区

        let file_part = Part::stream(buffer); // 创建一个 multipart 部分，包含文件内容
        let form = Form::new()
            .text("reqtype", "fileupload") // 表单参数：请求类型为文件上传
            .text("userhash", self.userhash.clone()) // 表单参数：用户哈希
            .part("fileToUpload", file_part); // 附加文件部分

        // 发起上传请求
        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?; // 异步发送请求

        if res.status().is_success() {
            let text = res.text().await?; // 返回的是文件 URL 文本
            Ok(text)
        } else {
            Err(anyhow!("Failed to upload file")) // 上传失败时返回错误
        }
    }

    // 上传 URL 到 Catbox
    pub async fn upload_url(&self, image_url: &str) -> Result<String> {
        let form = Form::new()
            .text("reqtype", "urlupload") // 请求类型：URL 上传
            .text("userhash", self.userhash.clone())
            .text("url", image_url.to_string());

        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?;

        if res.status().is_success() {
            let text = res.text().await?; // 返回的是文件 URL 文本
            Ok(text)
        } else {
            Err(anyhow!("Failed to upload URL")) // 上传失败时返回错误
        }
    }

    // 创建专辑并上传图片
    pub async fn create_album(&self, gallery_name: &str, description: &str, file_urls: Vec<String>) -> Result<String> {
        // 组织要上传到专辑的文件列表，最多 500 个文件
        let files = file_urls.join("\n"); // 文件列表以换行符分隔

        let form = Form::new()
            .text("reqtype", "createalbum")
            .text("userhash", self.userhash.clone())
            .text("title", gallery_name.to_string())
            .text("desc", description.to_string())
            .text("files", files);

        // 发起请求创建专辑
        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?;

        if res.status().is_success() {
            let text = res.text().await?; // 返回的是专辑短链接
            Ok(text)
        } else {
            Err(anyhow!("Failed to create album")) // 创建专辑失败时返回错误
        }
    }

    // 编辑专辑，添加新的图片
    pub async fn edit_album(&self, short: &str, gallery_name: &str, description: &str, file_urls: Vec<String>) -> Result<()> {
        let files = file_urls.join("\n"); // 文件 URL 列表以换行符分隔

        let form = Form::new()
            .text("reqtype", "editalbum")
            .text("userhash", self.userhash.clone())
            .text("short", short.to_string())
            .text("title", gallery_name.to_string())
            .text("desc", description.to_string())
            .text("files", files);

        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?;

        if res.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!("Failed to edit album")) // 编辑专辑失败时返回错误
        }
    }
}