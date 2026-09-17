use chrono::{DateTime, SubsecRound, Utc};
use tokio::runtime::Handle;
use tokio::task::block_in_place;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, NoTls, Row};
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

/// A query parameter, in the only three shapes a note's columns ever need.
/// Unlike `&(dyn ToSql + Sync)`, a unit test can build one of these without
/// a driver in sight.
#[derive(Debug, Clone, PartialEq)]
enum Param {
    Uuid(Uuid),
    Text(String),
    Timestamp(DateTime<Utc>),
}

impl Param {
    fn as_sql(&self) -> &(dyn ToSql + Sync) {
        match self {
            Param::Uuid(value) => value,
            Param::Text(value) => value,
            Param::Timestamp(value) => value,
        }
    }
}

/// A row as `PostgresStore`'s own logic needs it, independent of
/// `tokio_postgres::Row` — which nothing outside a real connection can
/// build, and so cannot cross into a unit test. Converting a driver row
/// into one of these is the only place in this file a test cannot reach;
/// everything downstream of it can.
#[derive(Debug, Clone, PartialEq)]
struct NoteRow {
    id: Uuid,
    title: String,
    body: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<Row> for NoteRow {
    fn from(row: Row) -> Self {
        NoteRow {
            id: row.get(0),
            title: row.get(1),
            body: row.get(2),
            created_at: row.get(3),
            updated_at: row.get(4),
        }
    }
}

impl From<NoteRow> for Note {
    fn from(row: NoteRow) -> Self {
        Note {
            id: row.id,
            title: row.title,
            body: row.body,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// The seam: a statement and its parameters go in, rows or a count this
/// module owns come back. `PostgresStore` is written entirely against this
/// trait, so a unit test can stand in for it and prove everything except
/// the one thing behind `PostgresClient` below — bytes actually crossing a
/// socket to a real PostgreSQL.
trait Connection: Send + Sync {
    fn query_opt(&self, statement: &str, params: &[Param]) -> Result<Option<NoteRow>, StoreError>;
    fn query(&self, statement: &str, params: &[Param]) -> Result<Vec<NoteRow>, StoreError>;
    fn execute(&self, statement: &str, params: &[Param]) -> Result<u64, StoreError>;
}

/// The real `Connection`. Everything here is the call itself: driving
/// `tokio_postgres` and converting what it hands back — proving that this
/// is correct against a real database is the integrator's system tests, not
/// a unit test in this file.
struct PostgresClient {
    client: Client,
}

impl Connection for PostgresClient {
    fn query_opt(&self, statement: &str, params: &[Param]) -> Result<Option<NoteRow>, StoreError> {
        block_in_place(|| {
            Handle::current().block_on(async {
                let args: Vec<&(dyn ToSql + Sync)> = params.iter().map(Param::as_sql).collect();
                let row = self
                    .client
                    .query_opt(statement, &args)
                    .await
                    .map_err(|err| StoreError(format!("querying notes: {err}")))?;

                Ok(row.map(NoteRow::from))
            })
        })
    }

    fn query(&self, statement: &str, params: &[Param]) -> Result<Vec<NoteRow>, StoreError> {
        block_in_place(|| {
            Handle::current().block_on(async {
                let args: Vec<&(dyn ToSql + Sync)> = params.iter().map(Param::as_sql).collect();
                let rows = self
                    .client
                    .query(statement, &args)
                    .await
                    .map_err(|err| StoreError(format!("querying notes: {err}")))?;

                Ok(rows.into_iter().map(NoteRow::from).collect())
            })
        })
    }

    fn execute(&self, statement: &str, params: &[Param]) -> Result<u64, StoreError> {
        block_in_place(|| {
            Handle::current().block_on(async {
                let args: Vec<&(dyn ToSql + Sync)> = params.iter().map(Param::as_sql).collect();
                self.client
                    .execute(statement, &args)
                    .await
                    .map_err(|err| StoreError(format!("running a statement: {err}")))
            })
        })
    }
}

/// The `Store` trait on PostgreSQL. The trait's methods are synchronous —
/// on purpose, so the API layer never has to know which store it is
/// talking to — but the driver underneath is not; `PostgresClient` hands
/// the current worker thread to Tokio's blocking pool for each call's
/// duration, so `Handle::current().block_on` drives the request without
/// nesting a runtime inside the one already polling this handler, which is
/// what a bare `.block_on` (or a sync driver built the same way) would
/// panic on.
pub struct PostgresStore {
    conn: Box<dyn Connection>,
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

        let migrated = async {
            client
                .batch_execute(include_str!("../../migrations/0001_create_notes.sql"))
                .await?;
            client
                .batch_execute(include_str!("../../migrations/0002_add_updated_at.sql"))
                .await
        }
        .await;

        client
            .execute("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_KEY])
            .await
            .map_err(|err| StoreError(format!("unlocking after migrations: {err}")))?;

        migrated.map_err(|err| StoreError(format!("running migrations: {err}")))?;

        Ok(Self {
            conn: Box::new(PostgresClient { client }),
        })
    }
}

impl Store for PostgresStore {
    fn create(&self, title: String, body: String) -> Result<Note, StoreError> {
        // `TIMESTAMPTZ` keeps microseconds, not nanoseconds: truncated
        // here, so the note this call returns already matches what a
        // later `get` or `list` reads back, rather than differing from it
        // by whatever Postgres would have dropped.
        let now = chrono::Utc::now().trunc_subsecs(6);
        let note = Note {
            id: Uuid::new_v4(),
            title,
            body,
            created_at: now,
            updated_at: now,
        };

        self.conn.execute(
            "INSERT INTO notes (id, title, body, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)",
            &[
                Param::Uuid(note.id),
                Param::Text(note.title.clone()),
                Param::Text(note.body.clone()),
                Param::Timestamp(note.created_at),
                Param::Timestamp(note.updated_at),
            ],
        )?;

        Ok(note)
    }

    fn get(&self, id: Uuid) -> Result<Option<Note>, StoreError> {
        let row = self.conn.query_opt(
            "SELECT id, title, body, created_at, updated_at FROM notes WHERE id = $1",
            &[Param::Uuid(id)],
        )?;

        Ok(row.map(Note::from))
    }

    fn list(&self) -> Result<Vec<Note>, StoreError> {
        let rows = self.conn.query(
            "SELECT id, title, body, created_at, updated_at FROM notes ORDER BY seq DESC",
            &[],
        )?;

        Ok(rows.into_iter().map(Note::from).collect())
    }

    fn update(&self, id: Uuid, title: String, body: String) -> Result<Option<Note>, StoreError> {
        // Truncated for the same reason as `create`'s `created_at`: what
        // this call returns must match what a later `get` reads back from
        // `TIMESTAMPTZ`.
        let updated_at = chrono::Utc::now().trunc_subsecs(6);

        let row = self.conn.query_opt(
            "UPDATE notes SET title = $2, body = $3, updated_at = $4 \
             WHERE id = $1 \
             RETURNING id, title, body, created_at, updated_at",
            &[
                Param::Uuid(id),
                Param::Text(title),
                Param::Text(body),
                Param::Timestamp(updated_at),
            ],
        )?;

        Ok(row.map(Note::from))
    }

    fn delete(&self, id: Uuid) -> Result<bool, StoreError> {
        let deleted = self
            .conn
            .execute("DELETE FROM notes WHERE id = $1", &[Param::Uuid(id)])?;

        Ok(rows_were_deleted(deleted))
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
    use std::sync::{Arc, Mutex};

    use chrono::TimeZone;

    use super::*;

    #[test]
    fn zero_affected_rows_means_nothing_was_deleted() {
        assert!(!rows_were_deleted(0));
    }

    #[test]
    fn one_or_more_affected_rows_means_something_was_deleted() {
        assert!(rows_were_deleted(1));
        assert!(rows_were_deleted(2));
    }

    fn some_time(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).unwrap()
    }

    fn some_row(id: Uuid, seconds: i64) -> NoteRow {
        NoteRow {
            id,
            title: "a title".to_string(),
            body: "a body".to_string(),
            created_at: some_time(seconds),
            updated_at: some_time(seconds),
        }
    }

    /// A statement and the parameters it was called with.
    type Call = (String, Vec<Param>);

    /// Every call made against a `StubConnection` lands here, so a test can
    /// check the statement and parameters the store built even after the
    /// connection has been moved into the store behind `Box<dyn Connection>`.
    #[derive(Clone, Default)]
    struct CallLog(Arc<Mutex<Vec<Call>>>);

    impl CallLog {
        fn record(&self, statement: &str, params: &[Param]) {
            self.0
                .lock()
                .unwrap()
                .push((statement.to_string(), params.to_vec()));
        }

        fn last(&self) -> Call {
            self.0
                .lock()
                .unwrap()
                .last()
                .cloned()
                .expect("no call was recorded")
        }
    }

    /// A stand-in `Connection`: each result a test needs is filled in, and
    /// every call is recorded in `calls`. A method whose result is left
    /// `None` panics if called, so a test asserting on `execute` cannot
    /// pass by accident through an unexamined `query`.
    #[derive(Default)]
    struct StubConnection {
        query_opt_result: Option<Result<Option<NoteRow>, StoreError>>,
        query_result: Option<Result<Vec<NoteRow>, StoreError>>,
        execute_result: Option<Result<u64, StoreError>>,
        calls: CallLog,
    }

    impl Connection for StubConnection {
        fn query_opt(
            &self,
            statement: &str,
            params: &[Param],
        ) -> Result<Option<NoteRow>, StoreError> {
            self.calls.record(statement, params);
            match &self.query_opt_result {
                Some(Ok(row)) => Ok(row.clone()),
                Some(Err(err)) => Err(StoreError(err.0.clone())),
                None => panic!("query_opt was not stubbed"),
            }
        }

        fn query(&self, statement: &str, params: &[Param]) -> Result<Vec<NoteRow>, StoreError> {
            self.calls.record(statement, params);
            match &self.query_result {
                Some(Ok(rows)) => Ok(rows.clone()),
                Some(Err(err)) => Err(StoreError(err.0.clone())),
                None => panic!("query was not stubbed"),
            }
        }

        fn execute(&self, statement: &str, params: &[Param]) -> Result<u64, StoreError> {
            self.calls.record(statement, params);
            match &self.execute_result {
                Some(Ok(count)) => Ok(*count),
                Some(Err(err)) => Err(StoreError(err.0.clone())),
                None => panic!("execute was not stubbed"),
            }
        }
    }

    fn store_on(conn: StubConnection) -> PostgresStore {
        PostgresStore {
            conn: Box::new(conn),
        }
    }

    #[test]
    fn get_gives_some_note_when_a_row_comes_back() {
        let id = Uuid::new_v4();
        let store = store_on(StubConnection {
            query_opt_result: Some(Ok(Some(some_row(id, 1)))),
            ..Default::default()
        });

        let note = store.get(id).unwrap();

        assert_eq!(note, Some(Note::from(some_row(id, 1))));
    }

    #[test]
    fn get_gives_none_when_no_row_comes_back() {
        let id = Uuid::new_v4();
        let store = store_on(StubConnection {
            query_opt_result: Some(Ok(None)),
            ..Default::default()
        });

        assert_eq!(store.get(id).unwrap(), None);
    }

    #[test]
    fn get_sends_the_id_as_its_only_parameter() {
        let id = Uuid::new_v4();
        let calls = CallLog::default();
        let store = store_on(StubConnection {
            query_opt_result: Some(Ok(None)),
            calls: calls.clone(),
            ..Default::default()
        });

        store.get(id).unwrap();

        let (statement, params) = calls.last();
        assert!(statement.contains("SELECT"));
        assert!(statement.contains("WHERE id = $1"));
        assert_eq!(params, vec![Param::Uuid(id)]);
    }

    #[test]
    fn get_propagates_a_driver_error_instead_of_a_silent_none() {
        let id = Uuid::new_v4();
        let store = store_on(StubConnection {
            query_opt_result: Some(Err(StoreError("connection reset".to_string()))),
            ..Default::default()
        });

        let error = store.get(id).unwrap_err();

        assert_eq!(error.to_string(), "connection reset");
    }

    #[test]
    fn list_gives_every_row_mapped_in_the_order_they_arrived() {
        let first = some_row(Uuid::new_v4(), 1);
        let second = some_row(Uuid::new_v4(), 2);
        let store = store_on(StubConnection {
            query_result: Some(Ok(vec![first.clone(), second.clone()])),
            ..Default::default()
        });

        let notes = store.list().unwrap();

        assert_eq!(notes, vec![Note::from(first), Note::from(second)]);
    }

    #[test]
    fn list_gives_nothing_when_no_rows_come_back() {
        let store = store_on(StubConnection {
            query_result: Some(Ok(vec![])),
            ..Default::default()
        });

        assert_eq!(store.list().unwrap(), vec![]);
    }

    #[test]
    fn list_sends_no_parameters() {
        let calls = CallLog::default();
        let store = store_on(StubConnection {
            query_result: Some(Ok(vec![])),
            calls: calls.clone(),
            ..Default::default()
        });

        store.list().unwrap();

        let (statement, params) = calls.last();
        assert!(statement.contains("SELECT"));
        assert_eq!(params, vec![]);
    }

    #[test]
    fn list_propagates_a_driver_error() {
        let store = store_on(StubConnection {
            query_result: Some(Err(StoreError("connection reset".to_string()))),
            ..Default::default()
        });

        assert_eq!(store.list().unwrap_err().to_string(), "connection reset");
    }

    #[test]
    fn update_gives_back_the_note_as_it_now_stands() {
        let id = Uuid::new_v4();
        let row = some_row(id, 5);
        let store = store_on(StubConnection {
            query_opt_result: Some(Ok(Some(row.clone()))),
            ..Default::default()
        });

        let note = store
            .update(id, "new title".to_string(), "new body".to_string())
            .unwrap();

        assert_eq!(note, Some(Note::from(row)));
    }

    #[test]
    fn update_gives_none_for_an_id_nothing_holds() {
        let id = Uuid::new_v4();
        let store = store_on(StubConnection {
            query_opt_result: Some(Ok(None)),
            ..Default::default()
        });

        let note = store
            .update(id, "new title".to_string(), "new body".to_string())
            .unwrap();

        assert_eq!(note, None);
    }

    #[test]
    fn update_sends_the_id_and_the_new_title_and_body() {
        let id = Uuid::new_v4();
        let calls = CallLog::default();
        let store = store_on(StubConnection {
            query_opt_result: Some(Ok(None)),
            calls: calls.clone(),
            ..Default::default()
        });

        store
            .update(id, "new title".to_string(), "new body".to_string())
            .unwrap();

        let (statement, params) = calls.last();
        assert!(statement.contains("UPDATE"));
        assert!(statement.contains("RETURNING"));
        assert_eq!(params[0], Param::Uuid(id));
        assert_eq!(params[1], Param::Text("new title".to_string()));
        assert_eq!(params[2], Param::Text("new body".to_string()));
    }

    #[test]
    fn update_propagates_a_driver_error() {
        let id = Uuid::new_v4();
        let store = store_on(StubConnection {
            query_opt_result: Some(Err(StoreError("connection reset".to_string()))),
            ..Default::default()
        });

        let error = store
            .update(id, "t".to_string(), "b".to_string())
            .unwrap_err();

        assert_eq!(error.to_string(), "connection reset");
    }

    #[test]
    fn delete_says_true_when_a_row_was_affected() {
        let id = Uuid::new_v4();
        let store = store_on(StubConnection {
            execute_result: Some(Ok(1)),
            ..Default::default()
        });

        assert!(store.delete(id).unwrap());
    }

    #[test]
    fn delete_says_false_when_no_row_was_affected() {
        let id = Uuid::new_v4();
        let store = store_on(StubConnection {
            execute_result: Some(Ok(0)),
            ..Default::default()
        });

        assert!(!store.delete(id).unwrap());
    }

    #[test]
    fn delete_sends_the_id_as_its_only_parameter() {
        let id = Uuid::new_v4();
        let calls = CallLog::default();
        let store = store_on(StubConnection {
            execute_result: Some(Ok(0)),
            calls: calls.clone(),
            ..Default::default()
        });

        store.delete(id).unwrap();

        let (statement, params) = calls.last();
        assert!(statement.contains("DELETE"));
        assert_eq!(params, vec![Param::Uuid(id)]);
    }

    #[test]
    fn delete_propagates_a_driver_error() {
        let id = Uuid::new_v4();
        let store = store_on(StubConnection {
            execute_result: Some(Err(StoreError("connection reset".to_string()))),
            ..Default::default()
        });

        assert_eq!(
            store.delete(id).unwrap_err().to_string(),
            "connection reset"
        );
    }

    #[test]
    fn create_sends_the_note_it_returns_as_its_parameters() {
        let calls = CallLog::default();
        let store = store_on(StubConnection {
            execute_result: Some(Ok(1)),
            calls: calls.clone(),
            ..Default::default()
        });

        let note = store
            .create("a title".to_string(), "a body".to_string())
            .unwrap();

        assert_eq!(note.title, "a title");
        assert_eq!(note.body, "a body");
        assert_eq!(note.created_at, note.updated_at);

        let (statement, params) = calls.last();
        assert!(statement.contains("INSERT"));
        assert_eq!(
            params,
            vec![
                Param::Uuid(note.id),
                Param::Text("a title".to_string()),
                Param::Text("a body".to_string()),
                Param::Timestamp(note.created_at),
                Param::Timestamp(note.updated_at),
            ]
        );
    }

    #[test]
    fn create_propagates_a_driver_error() {
        let store = store_on(StubConnection {
            execute_result: Some(Err(StoreError("connection reset".to_string()))),
            ..Default::default()
        });

        let error = store
            .create("a title".to_string(), "a body".to_string())
            .unwrap_err();

        assert_eq!(error.to_string(), "connection reset");
    }
}
