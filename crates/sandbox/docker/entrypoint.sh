#!/bin/bash
# Sandbox container entrypoint.
#
# This container is inert — it just sleeps until sandboxd sends
# commands via docker exec.  No agent, no repo cloning, no config.
set -e

echo "[sandbox] Container started. Waiting for instructions..."
exec /bin/sleep infinity
