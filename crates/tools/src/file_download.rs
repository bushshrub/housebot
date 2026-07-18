//! Download a public HTTP(S) file for delivery as a Discord attachment.

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::wait_for_slot;
use crate::web_fetch::validate_public_url;

const MAX_REDIRECTS: usize = 5;
const DOWNLOADS_PER_MINUTE: usize = 10;
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedFile {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

pub struct FileDownloader {
    client: Client,
    download_requests: Mutex<Vec<Instant>>,
}

impl Default for FileDownloader {
    fn default() -> Self {
        Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("Mozilla/5.0 (compatible; housebot/1.0)")
                .timeout(Duration::from_secs(45))
                .build()
                .expect("file download HTTP client should build"),
            download_requests: Mutex::new(Vec::new()),
        }
    }
}

impl FileDownloader {
    pub async fn download(
        &self,
        raw_url: &str,
        requested_filename: &str,
    ) -> Result<DownloadedFile, String> {
        wait_for_slot(&self.download_requests, DOWNLOADS_PER_MINUTE).await;
        let mut current = raw_url.to_string();
        let mut final_response = None;

        for _ in 0..=MAX_REDIRECTS {
            validate_public_url(&current)
                .await
                .map_err(|error| format!("Error: refusing to download {raw_url} ({error})"))?;
            let response = self
                .client
                .get(&current)
                .send()
                .await
                .map_err(|error| format!("Error: could not download file: {error}"))?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| format!("Error: redirect from {current} had no location"))?;
                let next = Url::parse(&current)
                    .and_then(|base| base.join(location))
                    .map_err(|_| format!("Error: invalid redirect from {current}"))?;
                current = next.to_string();
                continue;
            }
            final_response = Some(response);
            break;
        }

        let response = final_response
            .ok_or_else(|| format!("Error: too many redirects when downloading {raw_url}"))?;
        if response.status() != StatusCode::OK {
            return Err(format!(
                "Error: HTTP {} when downloading {raw_url}",
                response.status()
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_FILE_BYTES as u64)
        {
            return Err(format!(
                "Error: file is larger than the {} MiB attachment limit",
                MAX_FILE_BYTES / 1024 / 1024
            ));
        }

        let header_filename = response
            .headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            .and_then(filename_from_content_disposition);
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| format!("Error: could not read file: {error}"))?;
            if bytes.len().saturating_add(chunk.len()) > MAX_FILE_BYTES {
                return Err(format!(
                    "Error: file is larger than the {} MiB attachment limit",
                    MAX_FILE_BYTES / 1024 / 1024
                ));
            }
            bytes.extend_from_slice(&chunk);
        }

        let url_filename = Url::parse(&current).ok().and_then(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
        });
        let filename = [
            Some(requested_filename.to_string()).filter(|name| !name.trim().is_empty()),
            header_filename,
            url_filename,
        ]
        .into_iter()
        .flatten()
        .next()
        .map(|name| sanitize_filename(&name))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "download.bin".to_string());

        Ok(DownloadedFile {
            filename,
            bytes,
            content_type,
        })
    }
}

fn filename_from_content_disposition(header: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        key.eq_ignore_ascii_case("filename")
            .then(|| value.trim().trim_matches('"').to_string())
    })
}

fn sanitize_filename(filename: &str) -> String {
    let sanitized: String = filename
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
        .take(100)
        .collect();
    sanitized.trim_start_matches('.').to_string()
}

pub fn definition() -> Value {
    json!({
        "name": "download_file",
        "description": "Download a public HTTP(S) file and attach it directly to the Discord response. Use when the user asks to view, receive, or download a file found on the web. Private network URLs and files over 8 MiB are blocked.",
        "input_schema": {
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "Direct public URL of the file"},
                "filename": {"type": "string", "description": "Optional safe filename for the attachment"}
            },
            "required": ["url"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_requires_url() {
        assert_eq!(definition()["name"], "download_file");
        assert_eq!(definition()["input_schema"]["required"], json!(["url"]));
    }

    #[test]
    fn sanitizes_untrusted_filenames() {
        assert_eq!(
            sanitize_filename("../../my report (1).pdf"),
            "myreport1.pdf"
        );
        assert_eq!(sanitize_filename("safe_file-2.txt"), "safe_file-2.txt");
    }

    #[test]
    fn reads_basic_content_disposition_filename() {
        assert_eq!(
            filename_from_content_disposition("attachment; filename=\"report.pdf\""),
            Some("report.pdf".to_string())
        );
    }
}
