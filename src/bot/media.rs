//! Attachment/media extraction and reference-message context.

use std::sync::LazyLock;
use std::time::Duration;

use std::net::IpAddr;

use regex::Regex;
use reqwest::Url;

use super::*;

const GIF_FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const GIF_MAX_SIZE: usize = 10_000_000;

static GIF_URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://[^\s<>]+").unwrap());

pub(crate) async fn extract_media(msg: &Message) -> Vec<MediaData> {
    let mut media = Vec::new();
    for att in &msg.attachments {
        if is_pdf(&att.filename) {
            media.extend(extract_pdf_pages(&att.url, &att.filename).await);
            continue;
        }
        let Some(media_type) = media_type(&att.filename) else {
            continue;
        };
        if let Ok(resp) = MEDIA_CLIENT.get(&att.url).send().await {
            if let Ok(bytes) = resp.bytes().await {
                if bytes.len() > MEDIA_MAX_SIZE {
                    continue;
                }
                if media_type == "image/gif" {
                    media.extend(convert_gif_to_video(&bytes).await);
                } else {
                    use base64::Engine;
                    media.push(MediaData {
                        media_type: media_type.to_string(),
                        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    });
                }
            }
        }
    }
    media
}

/// Download GIF from links found in message text and convert to video.
pub(crate) async fn extract_gif_from_text(text: &str) -> Vec<MediaData> {
    let urls: Vec<String> = GIF_URL_RE
        .find_iter(text)
        .map(|m| {
            m.as_str()
                .trim_end_matches(['.', ',', '!', '?', ';', ':', ')', ']', '>', '&'])
                .to_string()
        })
        .filter(|url| {
            let lower = url.to_lowercase();
            lower.ends_with(".gif") || lower.contains(".gif?")
        })
        .filter(|url| is_safe_url(url))
        .collect();
    if urls.is_empty() {
        return Vec::new();
    }
    let client = reqwest::Client::builder()
        .timeout(GIF_FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default();
    let mut media = Vec::new();
    for url in urls {
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(bytes) = resp.bytes().await {
                if bytes.len() <= GIF_MAX_SIZE {
                    media.extend(convert_gif_to_video(&bytes).await);
                }
            }
        }
    }
    media
}

const MEDIA_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MEDIA_MAX_SIZE: usize = 25_000_000;

static MEDIA_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(MEDIA_FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("static reqwest client")
});

/// Basic URL safety check: must be http/https, not localhost, not a private IP.
fn is_safe_url(raw: &str) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let host = match url.host_str() {
        None => return false,
        Some(h) => h,
    };
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
    {
        return false;
    }
    let host_trimmed = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host_trimmed.parse::<IpAddr>() {
        match ip {
            IpAddr::V4(v4) => {
                if v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                {
                    return false;
                }
            }
            IpAddr::V6(v6) => {
                if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                    return false;
                }
            }
        }
    }
    true
}

/// Convert an animated GIF to an MP4 video using ffmpeg.
pub(crate) async fn convert_gif_to_video(bytes: &[u8]) -> Vec<MediaData> {
    let owned = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        let dir = std::env::temp_dir().join(format!("housebot-gif-{}", uuid::Uuid::new_v4()));
        let input = dir.join("input.gif");
        let output = dir.join("output.mp4");

        if std::fs::create_dir_all(&dir).is_err() {
            return Vec::new();
        }
        if std::fs::write(&input, &owned).is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return Vec::new();
        }

        let result = match std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                &input.to_string_lossy(),
                "-vf",
                "scale=max(2\\,trunc(iw/2)*2):max(2\\,trunc(ih/2)*2)",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
                &output.to_string_lossy(),
            ])
            .output()
        {
            Ok(out) if out.status.success() => match std::fs::read(&output) {
                Ok(video_bytes) => {
                    use base64::Engine;
                    vec![MediaData {
                        media_type: "video/mp4".to_string(),
                        data: base64::engine::general_purpose::STANDARD.encode(&video_bytes),
                    }]
                }
                Err(e) => {
                    tracing::warn!(%e, "Failed to read converted GIF video");
                    Vec::new()
                }
            },
            Ok(out) => {
                tracing::warn!(
                    stderr = %String::from_utf8_lossy(&out.stderr),
                    "ffmpeg GIF conversion failed"
                );
                Vec::new()
            }
            Err(e) => {
                tracing::warn!(%e, "Failed to execute ffmpeg for GIF conversion");
                Vec::new()
            }
        };

        let _ = std::fs::remove_dir_all(&dir);
        result
    })
    .await
    .unwrap_or_default()
}

pub(crate) const MAX_PDF_PAGES: usize = 10;

pub(crate) async fn extract_pdf_pages(url: &str, filename: &str) -> Vec<MediaData> {
    let Ok(response) = MEDIA_CLIENT.get(url).send().await else {
        return Vec::new();
    };
    let Ok(bytes) = response.bytes().await else {
        return Vec::new();
    };
    if bytes.len() > MEDIA_MAX_SIZE {
        return Vec::new();
    }

    let directory = std::env::temp_dir().join(format!("housebot-pdf-{}", Uuid::new_v4()));
    let input = directory.join("input.pdf");
    let output_prefix = directory.join("page");
    let result = async {
        tokio::fs::create_dir_all(&directory).await.ok()?;
        tokio::fs::write(&input, bytes).await.ok()?;
        let output = tokio::process::Command::new("pdftoppm")
            .args(pdf_render_arguments())
            .arg(&input)
            .arg(&output_prefix)
            .output()
            .await
            .ok()?;
        if !output.status.success() {
            tracing::warn!(%filename, stderr = %String::from_utf8_lossy(&output.stderr), "Failed to render PDF attachment");
            return None;
        }

        let mut pages = tokio::fs::read_dir(&directory).await.ok()?;
        let mut paths = Vec::new();
        while let Some(entry) = pages.next_entry().await.ok()? {
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "png") {
                paths.push(path);
            }
        }
        paths.sort();

        let mut media = Vec::with_capacity(paths.len());
        for path in paths {
            let page = tokio::fs::read(path).await.ok()?;
            use base64::Engine;
            media.push(MediaData {
                media_type: "image/png".to_string(),
                data: base64::engine::general_purpose::STANDARD.encode(page),
            });
        }
        Some(media)
    }
    .await
    .unwrap_or_default();
    let _ = tokio::fs::remove_dir_all(&directory).await;
    result
}

pub(crate) fn pdf_render_arguments() -> [String; 7] {
    [
        "-png".to_string(),
        "-r".to_string(),
        "144".to_string(),
        "-f".to_string(),
        "1".to_string(),
        "-l".to_string(),
        MAX_PDF_PAGES.to_string(),
    ]
}

pub(crate) fn is_pdf(filename: &str) -> bool {
    filename
        .rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("pdf"))
}

pub(crate) fn media_type(filename: &str) -> Option<&'static str> {
    match filename
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("mp3") => Some("audio/mpeg"),
        Some("wav") => Some("audio/wav"),
        Some("flac") => Some("audio/flac"),
        Some("mp4") => Some("video/mp4"),
        Some("mov") => Some("video/quicktime"),
        Some("webm") => Some("video/webm"),
        Some("mkv") => Some("video/x-matroska"),
        Some("avi") => Some("video/x-msvideo"),
        Some("m4v") => Some("video/x-m4v"),
        _ => None,
    }
}

pub(crate) fn message_has_attachments(msg: &Message) -> bool {
    !msg.attachments.is_empty()
}

pub(crate) fn message_attachment_context(msg: &Message) -> Option<String> {
    attachment_context(
        msg.attachments
            .iter()
            .map(|attachment| (attachment.filename.as_str(), attachment.url.as_str())),
    )
}

pub(crate) fn attachment_context<'a>(
    attachments: impl Iterator<Item = (&'a str, &'a str)>,
) -> Option<String> {
    let attachments: Vec<_> = attachments.collect();
    if attachments.is_empty() {
        return None;
    }

    let mut context = String::from(
        "[Attachments in this message. These files are already available; do not ask the user to upload them again.]",
    );
    for (filename, url) in attachments {
        context.push_str(&format!("\n- `{filename}`: {url}"));
    }
    Some(context)
}

pub(crate) fn referenced_message_context(msg: &Message) -> Option<String> {
    let content = msg.content.trim();
    let embed_text = msg
        .embeds
        .first()
        .and_then(|embed| embed.description.as_deref())
        .map(str::trim)
        .unwrap_or_default();
    let text = if content.is_empty() {
        embed_text
    } else {
        content
    };
    let urls: Vec<&str> = URL.find_iter(text).map(|m| m.as_str()).collect();
    let attachment_context = message_attachment_context(msg);
    if text.is_empty() && attachment_context.is_none() {
        return None;
    }

    let mut context = format!("[Message being replied to, id: {}]\n", msg.id);
    if !text.is_empty() {
        context.push_str(text);
    }
    if !urls.is_empty() {
        context.push_str(
            "\n\nThe message above contains URL(s). Use the web fetch tool on these URL(s) before answering: ",
        );
        context.push_str(&urls.join(", "));
    }
    if let Some(attachments) = attachment_context {
        context.push_str("\n\n");
        context.push_str(&attachments);
    }
    context.push_str("\n[End message being replied to]");
    Some(context)
}

pub(crate) fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod media_tests;
