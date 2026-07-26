#!/usr/bin/env bash
set -euo pipefail

mkdir -p integration-logs

cleanup() {
  if [[ -n "${HOUSEBOT_PID:-}" ]]; then
    kill "$HOUSEBOT_PID" 2>/dev/null || true
  fi
  if [[ -n "${MOCK_LLM_PID:-}" ]]; then
    kill "$MOCK_LLM_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

target/debug/discord-integration-tests mock-llm \
  >integration-logs/mock-llm.log 2>&1 &
MOCK_LLM_PID=$!

for _ in $(seq 1 30); do
  if grep -q "mock LLM listening" integration-logs/mock-llm.log; then
    break
  fi
  if ! kill -0 "$MOCK_LLM_PID" 2>/dev/null; then
    cat integration-logs/mock-llm.log
    exit 1
  fi
  sleep 1
done
grep -q "mock LLM listening" integration-logs/mock-llm.log

target/debug/housebot >integration-logs/housebot.log 2>&1 &
HOUSEBOT_PID=$!

for _ in $(seq 1 60); do
  if grep -q "Logged in as" integration-logs/housebot.log; then
    break
  fi
  if ! kill -0 "$HOUSEBOT_PID" 2>/dev/null; then
    cat integration-logs/housebot.log
    exit 1
  fi
  sleep 1
done
grep -q "Logged in as" integration-logs/housebot.log

target/debug/discord-integration-tests driver \
  2>&1 | tee integration-logs/driver.log
