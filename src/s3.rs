use crate::config::Catbox;
use anyhow::Result;
use reqwest::multipart;
use tokio::io::AsyncRead;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

pub struct CatboxUploader {
    upload_url: String,
    userhash: Option<String>,
}

impl CatboxUploader {
    pub fn new(catbox: &Catbox) -> Self {
        Self {
            upload_url: catbox.upload_url.clone(),
            userhash: catbox.userhash.clone(),
        }
    }

    /// Upload a file to Catbox
    pub async fn upload(&self, name: &str, file_path: &str) -> Result<String> {
        let client = reqwest::Client::new();

        // Create the multipart form data
        let form = multipart::Form::new()
            .text("reqtype", "fileupload")
            .text("userhash", self.userhash.clone().unwrap_or_default())
            .file("fileToUpload", file_path)?;

        // Send the POST request to the Catbox API
        let response = client.post(&self.upload_url).multipart(form).send().await?;

        // Check if the response is successful
        if response.status().is_success() {
            let body = response.text().await?;
            Ok(body) // Return the response body (uploaded file URL)
        } else {
            Err(anyhow::anyhow!(
                "Failed to upload file: {}",
                response.status()
            ))
        }
    }

    /// Upload a file using an async reader (if needed)
    pub async fn upload_from_reader<R: AsyncRead + Unpin>(
        &self,
        name: &str,
        reader: &mut R,
    ) -> Result<String> {
        // Save the reader content to a temporary file
        let temp_file_path = format!("/tmp/{}", name);
        let mut temp_file = File::create(&temp_file_path).await?;
        tokio::io::copy(reader, &mut temp_file).await?;

        // Call the `upload` method to upload the file
        let result = self.upload(name, &temp_file_path).await;

        // Clean up the temporary file
        tokio::fs::remove_file(temp_file_path).await?;

        result
    }
}
