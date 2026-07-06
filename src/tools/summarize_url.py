"""Agent tool for fetching and summarizing a web page."""

import asyncio
import re
from typing import Any

import aiohttp

MAX_CONTENT_CHARS = 8000
FETCH_TIMEOUT = 15

TOOL_DEFINITION: dict[str, Any] = {
    "name": "summarize_url",
    "description": (
        "Fetch the content of a public web page and return a concise summary. "
        "Use this when the user shares a URL and wants to know what it contains, "
        "or when a search result URL needs to be read in full."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "The full URL (including https://) to fetch and summarize.",
            },
        },
        "required": ["url"],
    },
}


async def fetch_and_summarize(url: str, llm_client: Any, model: str) -> str:
    try:
        async with aiohttp.ClientSession() as session:
            async with session.get(
                url,
                timeout=aiohttp.ClientTimeout(total=FETCH_TIMEOUT),
                headers={"User-Agent": "house-chatbot/1.0"},
                allow_redirects=True,
            ) as resp:
                if resp.status != 200:
                    return f"Error: HTTP {resp.status} when fetching {url}"
                raw = await resp.text(errors="replace")
    except aiohttp.ClientError as exc:
        return f"Error: could not fetch URL: {exc}"
    except asyncio.TimeoutError:
        return f"Error: timed out fetching {url}"

    text = re.sub(r"<[^>]+>", " ", raw)
    text = re.sub(r"\s+", " ", text).strip()
    truncated = text[:MAX_CONTENT_CHARS]

    prompt = (
        f"Summarize the following web page content in 3-5 sentences. "
        f"Focus on the most important information.\n\nURL: {url}\n\nCONTENT:\n{truncated}"
    )
    response = await llm_client.chat.completions.create(
        model=model,
        messages=[{"role": "user", "content": prompt}],
        max_tokens=512,
    )
    return response.choices[0].message.content or "(no summary generated)"
