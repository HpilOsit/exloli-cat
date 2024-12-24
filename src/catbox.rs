use reqwest::multipart::{Form, Part};
use reqwest::Client;
use anyhow::{Result, anyhow};
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone)]
pub struct CatboxUploader {
    userhash: String,  // Catbox 用户的 userhash
    client: Client,    // HTTP 客户端
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
            .multipart(form) // 使用 multipart 表单
            .send()
            .await?; // 异步发送请求

        if res.status().is_success() { // 如果响应成功
            let json: serde_json::Value = res.json().await?; // 解析 JSON 响应
            if let Some(url) = json["fileURL"].as_str() { // 从 JSON 中提取文件 URL
                Ok(url.to_string()) // 返回文件 URL
            } else {
                Err(anyhow!("Failed to get file URL from response")) // 如果没有找到 URL，则返回错误
            }
        } else {
            Err(anyhow!("Failed to upload file")) // 上传失败时返回错误
        }
    }

    // 上传 URL 到 Catbox
    pub async fn upload_url(&self, image_url: &str) -> Result<String> {
        let form = Form::new()
            .text("reqtype", "urlupload") // 请求类型：URL 上传
            .text("userhash", self.userhash.clone()) // 用户哈希
            .text("url", image_url.to_string()); // 上传的 URL

        // 发起请求
        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?; // 异步发送请求

        if res.status().is_success() { // 如果请求成功
            let json: serde_json::Value = res.json().await?; // 解析 JSON 响应
            if let Some(url) = json["fileURL"].as_str() { // 获取文件 URL
                Ok(url.to_string()) // 返回 URL
            } else {
                Err(anyhow!("Failed to get file URL from response")) // 没有 URL，则返回错误
            }
        } else {
            Err(anyhow!("Failed to upload URL")) // 上传失败时返回错误
        }
    }

    // 创建专辑并上传图片
    pub async fn create_album(&self, gallery_name: &str, description: &str, file_urls: Vec<String>) -> Result<String> {
        // 组织要上传到专辑的文件列表，最多 500 个文件
        let files = file_urls.join(" "); // 文件列表以空格分隔
        let album_name = gallery_name.to_string(); 
        let description = description.to_string(); 
        let file_data: Vec<u8> = files.into_bytes();  // 转换文件列表为字节数据

        let form = Form::new()
            .text("reqtype", "createalbum")
            .text("userhash", self.userhash.clone())
            .text("title", album_name)
            .text("desc", description)
            .part("fileToUpload", Part::bytes(file_data)); // 使用文件数据作为字节

        // 发起请求创建专辑
        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?; // 异步发送请求

        if res.status().is_success() { // 如果响应成功
            let json: serde_json::Value = res.json().await?; // 解析 JSON 响应
            if let Some(short) = json["short"].as_str() { // 获取专辑短链接
                Ok(short.to_string()) // 返回专辑短链接
            } else {
                Err(anyhow!("Failed to create album")) // 创建专辑失败
            }
        } else {
            Err(anyhow!("Failed to create album")) // 创建专辑失败
        }
    }

    // 编辑专辑，添加新的图片
    pub async fn edit_album(&self, short: &str, gallery_name: &str, description: &str, file_urls: Vec<String>) -> Result<()> {
        let files = file_urls.join(" "); // 文件列表

        let form = Form::new()
            .text("reqtype", "editalbum") 
            .text("userhash", self.userhash.clone()) 
            .text("short", short) 
            .text("title", gallery_name.to_string()) // 转换为 String
            .text("desc", description.to_string()) // 新的专辑描述
            .text("files", files); // 新的文件列表

        // 发起请求编辑专辑
        let res = self.client.post("https://catbox.moe/user/api.php")
            .multipart(form)
            .send()
            .await?;

        if !res.status().is_success() {
            Err(anyhow!("Failed to edit album")) // 编辑专辑失败
        } else {
            Ok(())
        }
    }
}
