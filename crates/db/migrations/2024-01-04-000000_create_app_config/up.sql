-- Initial schema creation for Diesel
-- This creates the app_config table for testing

CREATE TABLE IF NOT EXISTS app_config (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
