#!/bin/bash
set -e

# nginx binds port 3000 and runs as root (needs to start before dropping privs)
nginx

# Run the API server as the non-root sandbox user
exec su -s /bin/bash sandbox -c \
    '/api/venv/bin/uvicorn server:app --app-dir /api --host 0.0.0.0 --port 8080'
