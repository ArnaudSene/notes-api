use chrono::SubsecRound;
use tokio::runtime::Handle;
use tokio::task::block_in_place;
use tokio_postgres::{Client, NoTls};
use uuid::Uuid;

use crate::note::Note;
use crate::store::{Store, StoreError};

/// An arbitrary, fixed key for the advisory lock that serializes migrations:
/// `CREATE TABLE IF NOT EXISTS` is not actually safe against two connections
/// racing through it at once (both see the table missing, both try to
/// create it, one gets a duplicate-relation error) — which is exactly what
/// several system tests starting the service at the same time against a
/// freshly-lifted database will do.
const MIGRATION_LOCK_KEY: i64 = 0x6e6f7465732d6462u64 as i64;

/// The `Store` trait on PostgreSQL. The trait's methods are synchronous —
/// on purpose, so the API layer never has to know which store it is
/// talking to — but the driver underneath is not. `block_in_place` hands
/// the current worker thread to Tokio's blocking pool for the query's
/// duration, so `Handle::current().block_on` drives the request without
/// nesting a runtime inside the one already polling this handler, which is
/// what a bare `.block_on` (or a sync driver built the same way) would
/// panic on.
pub struct PostgresStore {
    client: Client,
}

impl PostgresStore {
    /// Connects and applies `migrations/0001_create_notes.sql`. That file is
    /// `CREATE TABLE IF NOT EXISTS`, so running it again — another call
    /// here, another restart of the service — changes nothing on a database
    /// that already has the table.
    pub async fn connect(database_url: &str) -> Result<Self, StoreError> {
        let (client, connection) = tokio_postgres::connect(database_url, NoTls)
            .await
            .map_err(|err| StoreError(format!("connecting to postgres: {err}")))?;

        // Drives the connection's I/O; without this task running, every
        // query on `client` would simply hang.
        tokio::spawn(async move {
            if let Err(err) = connection.await {
                eprintln!("notes-api: postgres connection error: {err}");
            }
        });

        client
            .execute("SELECT pg_advisory_lock($1)", &[&MIGRATION_LOCK_KEY])
            .await
            .map_err(|err| StoreError(format!("locking for migrations: {err}")))?;

        let migrated = client
            .batch_execute(include_str!("../../migrations/0001_create_notes.sql"))
            .await;

        client
            .execute("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_KEY])
            .await
            .map_err(|err| StoreError(format!("unlocking after migrations: {err}")))?;

        migrated.map_err(|err| StoreError(format!("running migrations: {err}")))?;

        Ok(Self { client })
    }
}

impl Store for PostgresStore {
    fn create(&self, title: String, body: String) -> Result<Note, StoreError> {
        block_in_place(|| {
            Handle::current().block_on(async {
                // `TIMESTAMPTZ` keeps microseconds, not nanoseconds:
                // truncated here, so the note this call returns already
                // matches what a later `get` or `list` reads back,
                // rather than differing from it by whatever Postgres
                // would have dropped.
                let now = chrono::Utc::now().trunc_subsecs(6);
                let note = Note {
                    id: Uuid::new_v4(),
                    title,
                    body,
                    created_at: now,
                    updated_at: now,
                };

                self.client
                    .execute(
                        "INSERT INTO notes (id, title, body, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)",
                        &[
                            &note.id,
                            &note.title,
                            &note.body,
                            &note.created_at,
                            &note.updated_at,
                        ],
                    )
                    .await
                    .map_err(|err| StoreError(format!("creating a note: {err}")))?;

                Ok(note)
            })
        })
    }

    fn get(&self, id: Uuid) -> Result<Option<Note>, StoreError> {
        block_in_place(|| {
            Handle::current().block_on(async {
                let row = self
                    .client
                    .query_opt(
                        "SELECT id, title, body, created_at, updated_at FROM notes WHERE id = $1",
                        &[&id],
                    )
                    .await
                    .map_err(|err| StoreError(format!("reading a note: {err}")))?;

                Ok(row.map(|row| Note {
                    id: row.get(0),
                    title: row.get(1),
                    body: row.get(2),
                    created_at: row.get(3),
                    updated_at: row.get(4),
                }))
            })
        })
    }

    fn list(&self) -> Result<Vec<Note>, StoreError> {
        block_in_place(|| {
            Handle::current().block_on(async {
                let rows = self
                    .client
                    .query(
                        "SELECT id, title, body, created_at, updated_at FROM notes ORDER BY seq DESC",
                        &[],
                    )
                    .await
                    .map_err(|err| StoreError(format!("listing notes: {err}")))?;

                Ok(rows
                    .into_iter()
                    .map(|row| Note {
                        id: row.get(0),
                        title: row.get(1),
                        body: row.get(2),
                        created_at: row.get(3),
                        updated_at: row.get(4),
                    })
                    .collect())
            })
        })
    }

    fn update(&self, id: Uuid, title: String, body: String) -> Result<Option<Note>, StoreError> {
        block_in_place(|| {
            Handle::current().block_on(async {
                // Truncated for the same reason as `create`'s `created_at`:
                // what this call returns must match what a later `get`
                // reads back from `TIMESTAMPTZ`.
                let updated_at = chrono::Utc::now().trunc_subsecs(6);

                let row = self
                    .client
                    .query_opt(
                        "UPDATE notes SET title = $2, body = $3, updated_at = $4 \
                         WHERE id = $1 \
                         RETURNING id, title, body, created_at, updated_at",
                        &[&id, &title, &body, &updated_at],
                    )
                    .await
                    .map_err(|err| StoreError(format!("updating a note: {err}")))?;

                Ok(row.map(|row| Note {
                    id: row.get(0),
                    title: row.get(1),
                    body: row.get(2),
                    created_at: row.get(3),
                    updated_at: row.get(4),
                }))
            })
        })
    }

    fn delete(&self, id: Uuid) -> Result<bool, StoreError> {
        block_in_place(|| {
            Handle::current().block_on(async {
                let deleted = self
                    .client
                    .execute("DELETE FROM notes WHERE id = $1", &[&id])
                    .await
                    .map_err(|err| StoreError(format!("deleting a note: {err}")))?;

                Ok(rows_were_deleted(deleted))
            })
        })
    }
}

/// Whether a `DELETE` actually removed a row. `execute` hands back how many
/// rows the statement touched; zero is the only count that means "no row
/// anywhere had this id" rather than "one was removed".
fn rows_were_deleted(row_count: u64) -> bool {
    row_count > 0
}

#[cfg(test)]
mod tests {
    use super::rows_were_deleted;

    #[test]
    fn zero_affected_rows_means_nothing_was_deleted() {
        assert!(!rows_were_deleted(0));
    }

    #[test]
    fn one_or_more_affected_rows_means_something_was_deleted() {
        assert!(rows_were_deleted(1));
        assert!(rows_were_deleted(2));
    }
}
