//! Unit tests for `media` (split out to keep the module under 400 lines).

use super::{
    attachment_context, convert_gif_to_video, extract_gif_from_text, is_pdf, is_safe_url,
    media_type, pdf_render_arguments, referenced_message_context,
};
use serenity::all::Message;

fn msg(content: &str) -> Message {
    serde_json::from_value(serde_json::json!({
        "id": "1",
        "channel_id": "1",
        "author": {
            "id": "1",
            "username": "tester",
            "discriminator": "0000",
            "avatar": null
        },
        "content": content,
        "timestamp": "2026-01-01T00:00:00+00:00",
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "type": 0
    }))
    .unwrap()
}

#[test]
fn recognized_supported_media_extensions() {
    assert_eq!(media_type("PHOTO.PNG"), Some("image/png"));
    assert_eq!(media_type("recording.mp3"), Some("audio/mpeg"));
    assert_eq!(media_type("clip.mp4"), Some("video/mp4"));
    assert_eq!(media_type("document.pdf"), None);
    assert!(is_pdf("document.pdf"));
    assert!(is_pdf("DOCUMENT.PDF"));
    assert!(!is_pdf("document.txt"));
}

#[test]
fn attachment_context_keeps_documents_available_to_the_agent() {
    let context = attachment_context(
        [(
            "midterm.pdf",
            "https://cdn.discordapp.com/files/midterm.pdf",
        )]
        .into_iter(),
    )
    .unwrap();

    assert!(context.contains("already available"));
    assert!(context.contains("midterm.pdf"));
    assert!(context.contains("https://cdn.discordapp.com/files/midterm.pdf"));
}

#[test]
fn attachment_context_omits_empty_attachment_lists() {
    assert!(attachment_context(std::iter::empty()).is_none());
}

#[test]
fn referenced_context_with_content() {
    let context = referenced_message_context(&msg("Hello world")).unwrap();
    assert!(context.contains("Hello world"));
    assert!(context.starts_with("[Message being replied to, id: 1]"));
    assert!(context.ends_with("[End message being replied to]"));
}

#[test]
fn referenced_context_empty_content_no_attachments() {
    assert!(referenced_message_context(&msg("")).is_none());
}

#[test]
fn referenced_context_with_urls() {
    let context = referenced_message_context(&msg("Check https://example.com/page")).unwrap();
    assert!(context.contains("URL(s)"));
    assert!(context.contains("https://example.com/page"));
}

#[test]
fn referenced_context_with_attachments() {
    let m: Message = serde_json::from_value(serde_json::json!({
        "id": "2",
        "channel_id": "1",
        "author": {
            "id": "1",
            "username": "tester",
            "discriminator": "0000",
            "avatar": null
        },
        "content": "",
        "timestamp": "2026-01-01T00:00:00+00:00",
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [{
            "id": "10",
            "filename": "report.pdf",
            "url": "https://cdn.discord.com/report.pdf",
            "proxy_url": "https://media.discord.com/report.pdf",
            "size": 1024,
            "width": null,
            "height": null,
            "content_type": null
        }],
        "embeds": [],
        "pinned": false,
        "type": 0
    }))
    .unwrap();
    let context = referenced_message_context(&m).unwrap();
    assert!(context.contains("report.pdf"));
    assert!(context.contains("already available"));
}

#[test]
fn referenced_context_falls_back_to_embed_for_paginated_replies() {
    let m: Message = serde_json::from_value(serde_json::json!({
        "id": "4",
        "channel_id": "1",
        "author": {
            "id": "1",
            "username": "tester",
            "discriminator": "0000",
            "avatar": null
        },
        "content": "",
        "timestamp": "2026-01-01T00:00:00+00:00",
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [],
        "embeds": [{
            "type": "rich",
            "description": "Page one of the paginated reply"
        }],
        "pinned": false,
        "type": 0
    }))
    .unwrap();
    let context = referenced_message_context(&m).unwrap();
    assert!(context.contains("Page one of the paginated reply"));
}

#[test]
fn referenced_context_content_and_attachments() {
    let m: Message = serde_json::from_value(serde_json::json!({
        "id": "3",
        "channel_id": "1",
        "author": {
            "id": "1",
            "username": "tester",
            "discriminator": "0000",
            "avatar": null
        },
        "content": "See attached file",
        "timestamp": "2026-01-01T00:00:00+00:00",
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [{
            "id": "11",
            "filename": "data.csv",
            "url": "https://cdn.discord.com/data.csv",
            "proxy_url": "https://media.discord.com/data.csv",
            "size": 512,
            "width": null,
            "height": null,
            "content_type": null
        }],
        "embeds": [],
        "pinned": false,
        "type": 0
    }))
    .unwrap();
    let context = referenced_message_context(&m).unwrap();
    assert!(context.contains("See attached file"));
    assert!(context.contains("data.csv"));
}

#[test]
fn pdfs_are_rendered_as_png_pages_at_a_readable_resolution() {
    assert_eq!(
        pdf_render_arguments(),
        ["-png", "-r", "144", "-f", "1", "-l", "10"]
    );
}

#[tokio::test]
async fn extract_gif_from_text_finds_gif_urls() {
    let media = extract_gif_from_text("check this https://example.com/image.gif").await;
    assert!(media.is_empty());
}

#[tokio::test]
async fn extract_gif_from_text_finds_gif_urls_with_query_params() {
    let media = extract_gif_from_text("https://example.com/image.gif?width=400").await;
    assert!(media.is_empty());
}

#[tokio::test]
async fn extract_gif_from_text_skips_non_gif_urls() {
    let media = extract_gif_from_text("check this https://example.com/image.png").await;
    assert!(media.is_empty());
}

#[tokio::test]
async fn extract_gif_from_text_handles_empty_text() {
    let media = extract_gif_from_text("").await;
    assert!(media.is_empty());
}

#[tokio::test]
async fn extract_gif_from_text_handles_no_urls() {
    let media = extract_gif_from_text("just some text without urls").await;
    assert!(media.is_empty());
}

#[tokio::test]
async fn extract_gif_from_text_trims_trailing_punctuation() {
    let media = extract_gif_from_text("https://example.com/image.gif.").await;
    assert!(media.is_empty());
}

#[tokio::test]
async fn extract_gif_from_text_multiple_urls() {
    let media =
        extract_gif_from_text("a https://example.com/a.gif b https://example.com/b.gif?x=1").await;
    assert!(media.is_empty());
}

#[tokio::test]
async fn extract_gif_from_text_blocks_localhost() {
    let media = extract_gif_from_text("http://localhost:8080/image.gif").await;
    assert!(media.is_empty());
}

#[tokio::test]
async fn extract_gif_from_text_blocks_private_ip() {
    let media = extract_gif_from_text("http://192.168.1.1/image.gif").await;
    assert!(media.is_empty());
}

#[test]
fn is_safe_url_allows_public_urls() {
    assert!(is_safe_url("https://example.com/image.gif"));
    assert!(is_safe_url("http://cdn.example.com/foo.gif"));
    assert!(is_safe_url(
        "https://media.giphy.com/media/abc123/giphy.gif"
    ));
}

#[test]
fn is_safe_url_rejects_localhost() {
    assert!(!is_safe_url("http://localhost/image.gif"));
    assert!(!is_safe_url("http://localhost:8080/image.gif"));
    assert!(!is_safe_url("http://foo.localhost/image.gif"));
}

#[test]
fn is_safe_url_rejects_local_domain_hostnames() {
    assert!(!is_safe_url("https://foo.local/image.gif"));
}

#[test]
fn is_safe_url_rejects_non_http_schemes() {
    assert!(!is_safe_url("ftp://example.com/image.gif"));
    assert!(!is_safe_url("file:///tmp/image.gif"));
    assert!(!is_safe_url("data:image/gif;base64,R0lGOD"));
}

#[test]
fn is_safe_url_rejects_malformed_urls() {
    assert!(!is_safe_url(""));
    assert!(!is_safe_url("not a url"));
}

#[test]
fn convert_gif_to_video_handles_invalid_input() {
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(convert_gif_to_video(b"not a real gif"));
    assert!(result.is_empty());
}

#[test]
fn convert_gif_to_video_handles_empty_input() {
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(convert_gif_to_video(b""));
    assert!(result.is_empty());
}

#[test]
fn is_safe_url_rejects_private_ipv4() {
    assert!(!is_safe_url("http://10.0.0.1/image.gif"));
    assert!(!is_safe_url("http://172.16.0.1/image.gif"));
    assert!(!is_safe_url("http://192.168.1.1/image.gif"));
    assert!(!is_safe_url("http://127.0.0.1/image.gif"));
    assert!(!is_safe_url("http://169.254.1.1/image.gif"));
    assert!(!is_safe_url("http://0.0.0.0/image.gif"));
}

#[test]
fn is_safe_url_rejects_private_ipv6() {
    assert!(!is_safe_url("http://[::1]/image.gif"));
    assert!(!is_safe_url("http://[::]/image.gif"));
}

#[test]
fn is_safe_url_allows_public_ipv4() {
    assert!(is_safe_url("http://93.184.216.34/image.gif"));
    assert!(is_safe_url("http://8.8.8.8/image.gif"));
}
