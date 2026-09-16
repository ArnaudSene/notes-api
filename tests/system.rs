//! System tests: the compiled `notes-api` binary, run as its own process
//! against the real PostgreSQL of `compose.yaml`'s `db` service. Every test
//! here needs that database, so every one carries
//! `#[ignore = "system test: needs the services"]` — see
//! `/work/stack/system.sh`, which runs exactly these and only these; a plain
//! `cargo test` skips the lot.
//!
//! The database is not reset between tests, or between runs of this suite —
//! nunki never stops it between two profiles, so a fixture from a previous
//! run can still be there. Nothing here assumes its notes are the only ones
//! in the table, only that the ones it just created come back, in the right
//! place relative to each other.

#[path = "system/support.rs"]
mod support;

use support::{Service, database_url, unique_port};

#[test]
#[ignore = "system test: needs the services"]
fn a_note_survives_a_restart_of_the_service() {
    let db_url = database_url();
    let token = "system-restart-token";

    let created = {
        let service = Service::start(&db_url, token, unique_port());
        let (status, note) = service.post_note("groceries", "milk, eggs");
        assert_eq!(status, 201);
        note
        // `service` is dropped here: the process that created the note is
        // killed before the next one starts.
    };

    let service = Service::start(&db_url, token, unique_port());
    let id = created["id"].as_str().expect("the created note has an id");
    let (status, fetched) = service.get_note(id);

    assert_eq!(status, 200);
    assert_eq!(fetched, created);
}

#[test]
#[ignore = "system test: needs the services"]
fn two_notes_come_back_in_the_right_order() {
    let service = Service::start(&database_url(), "system-order-token", unique_port());

    let (status, first) = service.post_note("order-first", "1");
    assert_eq!(status, 201);
    let (status, second) = service.post_note("order-second", "2");
    assert_eq!(status, 201);

    let (status, notes) = service.list_notes();
    assert_eq!(status, 200);
    let notes = notes.as_array().expect("a JSON array");

    let index_of = |id: &serde_json::Value| {
        notes
            .iter()
            .position(|note| &note["id"] == id)
            .unwrap_or_else(|| panic!("note {id} missing from the list"))
    };

    assert!(
        index_of(&second["id"]) < index_of(&first["id"]),
        "the note created second must come back before the one created first"
    );
}

#[test]
#[ignore = "system test: needs the services"]
fn the_same_title_twice_does_not_collide() {
    let service = Service::start(&database_url(), "system-collision-token", unique_port());

    let (status, first) = service.post_note("duplicate-title", "one");
    assert_eq!(status, 201);
    let (status, second) = service.post_note("duplicate-title", "two");
    assert_eq!(status, 201);

    assert_ne!(first["id"], second["id"]);

    let (status, notes) = service.list_notes();
    assert_eq!(status, 200);
    let notes = notes.as_array().expect("a JSON array");

    let find = |id: &serde_json::Value| {
        notes
            .iter()
            .find(|note| &note["id"] == id)
            .unwrap_or_else(|| panic!("note {id} missing from the list"))
    };

    assert_eq!(find(&first["id"])["body"], "one");
    assert_eq!(find(&second["id"])["body"], "two");
}

#[test]
#[ignore = "system test: needs the services"]
fn migrations_apply_cleanly_twice_in_a_row() {
    let runtime = tokio::runtime::Runtime::new().expect("build a runtime for this test");
    let url = database_url();

    runtime
        .block_on(notes_api::store::postgres::PostgresStore::connect(&url))
        .expect("migrating the database once");
    runtime
        .block_on(notes_api::store::postgres::PostgresStore::connect(&url))
        .expect("migrating the same, already-migrated database again");
}
