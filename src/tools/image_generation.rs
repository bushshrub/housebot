//! Safety-gated image generation through an OpenAI-compatible API.

use std::time::{Duration, Instant};

use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::config;
use crate::tools::wait_for_slot;

const GENERATIONS_PER_MINUTE: usize = 5;
const MAX_PROMPT_CHARS: usize = 4_000;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const ALLOWED_SIZES: &[&str] = &["auto", "1024x1024", "1536x1024", "1024x1536"];

pub enum GeneratedImage {
    Bytes {
        filename: String,
        bytes: Vec<u8>,
        revised_prompt: Option<String>,
    },
    Url {
        url: String,
        revised_prompt: Option<String>,
    },
}

pub struct ImageGenerator {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
    moderation_model: String,
    generation_requests: Mutex<Vec<Instant>>,
}

#[derive(Deserialize)]
struct ModerationResponse {
    #[serde(default)]
    results: Vec<ModerationResult>,
}

#[derive(Deserialize)]
struct ModerationResult {
    #[serde(default)]
    flagged: bool,
}

#[derive(Deserialize)]
struct ImageResponse {
    #[serde(default)]
    data: Vec<ImageData>,
}

#[derive(Deserialize)]
struct ImageData {
    b64_json: Option<String>,
    url: Option<String>,
    revised_prompt: Option<String>,
}

impl ImageGenerator {
    pub fn from_env() -> Self {
        let llm_base = config::env_or("LLM_BASE_URL", "http://server-slop:8080/v1");
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("image generation HTTP client should build"),
            base_url: config::env_or("IMAGE_API_BASE_URL", &llm_base)
                .trim_end_matches('/')
                .to_string(),
            api_key: config::env_or(
                "IMAGE_API_KEY",
                &config::env_or("LLM_API_KEY", "not-required"),
            ),
            model: config::env_or("IMAGE_MODEL", "gpt-image-1"),
            moderation_model: config::env_or("IMAGE_MODERATION_MODEL", "omni-moderation-latest"),
            generation_requests: Mutex::new(Vec::new()),
        }
    }

    pub async fn generate(&self, prompt: &str, size: &str) -> Result<GeneratedImage, String> {
        validate_prompt(prompt)?;
        let size = if ALLOWED_SIZES.contains(&size) {
            size
        } else {
            "1024x1024"
        };
        wait_for_slot(&self.generation_requests, GENERATIONS_PER_MINUTE).await;
        self.moderate(prompt).await?;

        let response = self
            .client
            .post(format!("{}/images/generations", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&json!({
                "model": self.model,
                "prompt": prompt,
                "n": 1,
                "size": size
            }))
            .send()
            .await
            .map_err(|error| format!("Error: image generation request failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Error: image generation returned HTTP {}",
                response.status()
            ));
        }
        let response: ImageResponse = response
            .json()
            .await
            .map_err(|error| format!("Error: invalid image generation response: {error}"))?;
        let image = response
            .data
            .into_iter()
            .next()
            .ok_or("Error: image generation returned no image")?;
        if let Some(encoded) = image.b64_json {
            if encoded.len() > (MAX_IMAGE_BYTES * 4 / 3) + 4 {
                return Err("Error: generated image exceeds the 8 MiB attachment limit".into());
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|error| format!("Error: invalid generated image data: {error}"))?;
            if bytes.len() > MAX_IMAGE_BYTES {
                return Err("Error: generated image exceeds the 8 MiB attachment limit".into());
            }
            let extension = image_extension(&bytes)
                .ok_or("Error: image provider returned an unsupported file type")?;
            return Ok(GeneratedImage::Bytes {
                filename: format!("generated-image.{extension}"),
                bytes,
                revised_prompt: image.revised_prompt,
            });
        }
        if let Some(url) = image.url.filter(|url| url.starts_with("https://")) {
            return Ok(GeneratedImage::Url {
                url,
                revised_prompt: image.revised_prompt,
            });
        }
        Err("Error: image provider returned neither image data nor a secure URL".into())
    }

    async fn moderate(&self, prompt: &str) -> Result<(), String> {
        let response = self
            .client
            .post(format!("{}/moderations", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&json!({"model": self.moderation_model, "input": prompt}))
            .send()
            .await
            .map_err(|error| {
                format!("Error: safety screening unavailable; image was not generated: {error}")
            })?;
        if !response.status().is_success() {
            return Err(format!(
                "Error: safety screening unavailable (HTTP {}); image was not generated",
                response.status()
            ));
        }
        let moderation: ModerationResponse = response.json().await.map_err(|error| {
            format!("Error: invalid safety screening response; image was not generated: {error}")
        })?;
        let result = moderation
            .results
            .first()
            .ok_or("Error: safety screening returned no result; image was not generated")?;
        if result.flagged {
            return Err(
                "Error: image request was blocked by the safety filter; image was not generated"
                    .into(),
            );
        }
        Ok(())
    }
}

fn validate_prompt(prompt: &str) -> Result<(), String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Error: image prompt cannot be empty".into());
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!(
            "Error: image prompt exceeds the {MAX_PROMPT_CHARS}-character limit"
        ));
    }

    // Fail fast for unambiguously unsafe requests. The API moderation check remains
    // mandatory and fail-closed, so this is defense in depth rather than the sole filter.
    let normalized = prompt.to_ascii_lowercase();
    const BLOCKED_PHRASES: &[&str] = &[
        "child pornography",
        "sexualized minor",
        "nude child",
        "non-consensual pornography",
        "revenge porn",
        "graphic torture",
        "instructions to self-harm",
        "suicide instructions",
    ];
    if BLOCKED_PHRASES
        .iter()
        .any(|blocked| normalized.contains(blocked))
    {
        return Err(
            "Error: image request was blocked by the safety filter; image was not generated".into(),
        );
    }
    Ok(())
}

fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else {
        None
    }
}

pub fn definition() -> Value {
    json!({
        "name": "generate_image",
        "description": "Generate one image in Discord after mandatory, fail-closed safety screening. Use only when the user explicitly asks for an image. Unsafe prompts are blocked.",
        "input_schema": {
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "description": "Detailed visual description of the requested image"},
                "size": {
                    "type": "string",
                    "enum": ["auto", "1024x1024", "1536x1024", "1024x1536"],
                    "default": "1024x1024"
                }
            },
            "required": ["prompt"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_requires_prompt() {
        assert_eq!(definition()["name"], "generate_image");
        assert_eq!(definition()["input_schema"]["required"], json!(["prompt"]));
    }

    #[test]
    fn local_safety_filter_blocks_unambiguously_unsafe_prompts() {
        assert!(validate_prompt("Create graphic torture of a real person").is_err());
        assert!(validate_prompt("A watercolor landscape at sunrise").is_ok());
    }

    #[test]
    fn recognizes_supported_image_data() {
        assert_eq!(image_extension(b"\x89PNG\r\n\x1a\nmore"), Some("png"));
        assert_eq!(image_extension(b"\xff\xd8\xffmore"), Some("jpg"));
        assert_eq!(image_extension(b"RIFFxxxxWEBPmore"), Some("webp"));
        assert_eq!(image_extension(b"GIF89a"), None);
    }
}
