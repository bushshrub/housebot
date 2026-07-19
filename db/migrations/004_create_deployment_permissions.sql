CREATE TABLE IF NOT EXISTS deployment_permissions (
    user_id BIGINT PRIMARY KEY,
    granted_by BIGINT NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
