-- Schema version 1 (WP-02).
-- Timestamps are RFC 3339 text in UTC. Booleans are INTEGER 0/1. Enums are their serde string.

CREATE TABLE accounts (
    id                    TEXT PRIMARY KEY NOT NULL,
    brand                 TEXT NOT NULL,
    username              TEXT NOT NULL,
    enabled               INTEGER NOT NULL,
    polling_interval_secs INTEGER NOT NULL,
    connection_state      TEXT NOT NULL,
    last_success_at       TEXT,
    last_error_at         TEXT,
    last_error_category   TEXT,
    last_error_message    TEXT,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);

CREATE TABLE vehicles (
    vin                   TEXT PRIMARY KEY NOT NULL,
    account_id            TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    brand                 TEXT NOT NULL,
    model                 TEXT,
    display_name          TEXT,
    drivetrain            TEXT NOT NULL,
    enabled               INTEGER NOT NULL,
    missing_since         TEXT,
    data_state            TEXT NOT NULL,
    last_update_at        TEXT,
    last_error_at         TEXT,
    last_error_category   TEXT,
    last_error_message    TEXT,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);

CREATE INDEX vehicles_account_id ON vehicles(account_id);

CREATE TABLE vehicle_states (
    vin                   TEXT PRIMARY KEY NOT NULL REFERENCES vehicles(vin) ON DELETE CASCADE,
    fetched_at            TEXT NOT NULL,
    state_json            TEXT NOT NULL,
    last_changes_json     TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);

CREATE TABLE mqtt_connections (
    id                    TEXT PRIMARY KEY NOT NULL,
    enabled               INTEGER NOT NULL,
    host                  TEXT NOT NULL,
    port                  INTEGER NOT NULL,
    username              TEXT,
    tls                   INTEGER NOT NULL,
    topic_prefix          TEXT NOT NULL,
    client_id             TEXT NOT NULL,
    keep_alive_secs       INTEGER NOT NULL,
    protocol              TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);

CREATE TABLE secrets (
    owner_kind            TEXT NOT NULL,
    owner_id              TEXT NOT NULL,
    kind                  TEXT NOT NULL,
    key_version           INTEGER NOT NULL,
    nonce                 BLOB NOT NULL,
    ciphertext            BLOB NOT NULL,
    updated_at            TEXT NOT NULL,
    PRIMARY KEY (owner_kind, owner_id, kind)
);

CREATE TABLE settings (
    key                   TEXT PRIMARY KEY NOT NULL,
    value_json            TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);
