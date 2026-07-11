//! Agent tool for translating text using the local LLM.

use serde_json::{json, Value};

use crate::llm::ChatClient;

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "translate",
        "description": "Translate text to a target language using the LLM. Source language is \
            auto-detected. Use this whenever a user asks to translate something.",
        "input_schema": {
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "The text to translate."},
                "target_language": {"type": "string", "description": "The language to translate into (e.g. 'French', 'Spanish', 'Japanese', 'German')."}
            },
            "required": ["text", "target_language"]
        }
    })
}

/// Build the translation prompt sent to the model.
pub fn build_prompt(text: &str, target_language: &str) -> String {
    format!(
        "Translate the following text to {target_language}. Return only the translation, with no \
         explanation or commentary.\n\nTEXT:\n{text}"
    )
}

/// Translate `text` into `target_language`, returning the translation or a fallback string.
pub async fn translate_text(
    client: &dyn ChatClient,
    model: &str,
    text: &str,
    target_language: &str,
) -> String {
    let messages = vec![json!({"role": "user", "content": build_prompt(text, target_language)})];
    match client.chat_once(model, &messages, 2048).await {
        Ok(out) if out.content.as_deref().is_some_and(|text| !text.is_empty()) => {
            out.content.unwrap_or_default()
        }
        _ => "(no translation generated)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockChatClient;

    #[tokio::test]
    async fn returns_translation() {
        let client = MockChatClient::new().with_once_reply("Bonjour le monde");
        let out = translate_text(&client, "m", "Hello world", "French").await;
        assert_eq!(out, "Bonjour le monde");
    }

    #[tokio::test]
    async fn prompt_includes_target_language() {
        let client = MockChatClient::new().with_once_reply("Hola");
        translate_text(&client, "m", "Hello", "Spanish").await;
        let calls = client.once_calls.lock().unwrap();
        let content = calls[0][0]["content"].as_str().unwrap();
        assert!(content.contains("Spanish"));
    }

    #[tokio::test]
    async fn prompt_includes_source_text() {
        let client = MockChatClient::new().with_once_reply("Ciao");
        translate_text(&client, "m", "Hello there", "Italian").await;
        let calls = client.once_calls.lock().unwrap();
        let content = calls[0][0]["content"].as_str().unwrap();
        assert!(content.contains("Hello there"));
    }

    #[tokio::test]
    async fn empty_response_returns_fallback() {
        let client = MockChatClient::new(); // empty reply
        let out = translate_text(&client, "m", "hello", "German").await;
        assert!(out.contains("no translation"));
    }

    #[test]
    fn definition_has_required_fields() {
        let d = definition();
        assert_eq!(d["name"], "translate");
        let props = &d["input_schema"]["properties"];
        assert!(props.get("text").is_some());
        assert!(props.get("target_language").is_some());
    }
}
