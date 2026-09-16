-- The notes table. Every statement here is safe to run again against a
-- database that already has it: the service (and the system tests) apply
-- this file at every startup, not just the first one.
CREATE TABLE IF NOT EXISTS notes (
    id UUID PRIMARY KEY,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    -- Insertion order, for "newest first": two notes can share a
    -- `created_at` at whatever resolution the clock gives, but never a
    -- `seq`.
    seq BIGSERIAL
);
