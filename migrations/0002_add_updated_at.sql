-- `0001` shipped without `updated_at`: the column `update` reads and writes
-- did not exist yet. Adds it, backfills existing rows from `created_at` (the
-- only value that means "unchanged since creation" for a row nothing has
-- touched), and only then makes it required — so this file, like `0001`,
-- is safe to run again against a database that already has it.
ALTER TABLE notes ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ;
UPDATE notes SET updated_at = created_at WHERE updated_at IS NULL;
ALTER TABLE notes ALTER COLUMN updated_at SET NOT NULL;
