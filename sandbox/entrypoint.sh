#!/bin/bash
set -e

cd /workspace

if [ -n "$REPO_URL" ]; then
    echo "[sandbox] Cloning $REPO_URL..."
    git clone --depth=1 "$REPO_URL" .
fi

LLAMA_BASE="${LLAMA_CPP_URL:-http://server-slop:8080}/v1"
LLAMA_MODEL="${LLAMA_CPP_MODEL:-gemma-4-12b-qat-q4kxl}"

cat > /workspace/opencode.json << OPENCODE_EOF
{
  "provider": {
    "server-slop": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "server-slop",
      "options": {
        "baseURL": "${LLAMA_BASE}",
        "apiKey": "not-required"
      },
      "models": {
        "gemma-4-12b-qat-q4kxl": {"name": "Gemma 4 12B",    "limit": {"context": 131072, "output": 8192}},
        "qwen3.6-35b":           {"name": "Qwen 3.6 35B",   "limit": {"context": 262144, "output": 65536}},
        "ornith-1.0-35b":        {"name": "Ornith 1.0 35B", "limit": {"context": 262144, "output": 65536}},
        "qwen3.6-27b":           {"name": "Qwen 3.6 27B",   "limit": {"context": 262144, "output": 65536}},
        "fastcontext-1.0":       {"name": "FastContext 1.0", "limit": {"context": 131072, "output": 4096}}
      }
    }
  },
  "permission": {
    "bash": {"*": "allow"}
  }
}
OPENCODE_EOF

case "$AGENT" in
  opencode)
    exec opencode run --dangerously-skip-permissions "$TASK" \
      --model="${MODEL:-server-slop/$LLAMA_MODEL}"
    ;;
  claude)
    exec claude --dangerously-skip-permissions -p "$TASK" \
      --model="${MODEL:-claude-haiku-4-5-20251001}"
    ;;
  *)
    echo "[sandbox] Unknown agent: $AGENT" >&2
    exit 1
    ;;
esac
