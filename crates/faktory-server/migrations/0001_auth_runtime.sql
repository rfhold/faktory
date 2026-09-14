-- Kuri and MCP apply their pinned browser-session and OAuth migrations first.
-- This marker makes the Faktory runtime schema version explicit in the same database.
CREATE TABLE IF NOT EXISTS faktory_auth_runtime_schema (
    schema_version INTEGER PRIMARY KEY,
    installed_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO faktory_auth_runtime_schema (schema_version)
VALUES (1)
ON CONFLICT (schema_version) DO NOTHING;
