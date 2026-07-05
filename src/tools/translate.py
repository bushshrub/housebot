"""Agent tool for translating text using the local LLM."""

from typing import Any

TOOL_DEFINITION: dict[str, Any] = {
    "name": "translate",
    "description": (
        "Translate text to a target language using the LLM. "
        "Source language is auto-detected. "
        "Use this whenever a user asks to translate something."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "text": {
                "type": "string",
                "description": "The text to translate.",
            },
            "target_language": {
                "type": "string",
                "description": "The language to translate into (e.g. 'French', 'Spanish', 'Japanese', 'German').",
            },
        },
        "required": ["text", "target_language"],
    },
}


async def translate_text(text: str, target_language: str, llm_client: Any, model: str) -> str:
    prompt = (
        f"Translate the following text to {target_language}. "
        f"Return only the translation, with no explanation or commentary.\n\nTEXT:\n{text}"
    )
    response = await llm_client.chat.completions.create(
        model=model,
        messages=[{"role": "user", "content": prompt}],
        max_tokens=2048,
    )
    return response.choices[0].message.content or "(no translation generated)"
