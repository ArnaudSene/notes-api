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

use support::{Service, database_url, encode_query_value, unique_marker, unique_port};

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
fn a_note_changed_through_one_process_is_read_back_changed_by_another() {
    let db_url = database_url();
    let token = "system-update-token";

    let updated = {
        let service = Service::start(&db_url, token, unique_port());
        let (status, created) = service.post_note("shopping", "milk");
        assert_eq!(status, 201);

        let id = created["id"].as_str().expect("the created note has an id");
        let (status, updated) = service.put_note(id, "shopping list", "milk, eggs");
        assert_eq!(status, 200);
        updated
        // `service` is dropped here: the process that wrote the change is
        // killed before the next one reads it back.
    };

    let service = Service::start(&db_url, token, unique_port());
    let id = updated["id"].as_str().expect("the updated note has an id");
    let (status, fetched) = service.get_note(id);

    assert_eq!(status, 200);
    assert_eq!(fetched, updated);
}

#[test]
#[ignore = "system test: needs the services"]
fn updated_at_moves_on_a_put_and_created_at_does_not() {
    let service = Service::start(&database_url(), "system-timestamps-token", unique_port());

    let (status, created) = service.post_note("draft", "v1");
    assert_eq!(status, 201);
    assert_eq!(created["created_at"], created["updated_at"]);

    let id = created["id"].as_str().expect("the created note has an id");
    let (status, updated) = service.put_note(id, "draft", "v2");
    assert_eq!(status, 200);

    assert_eq!(
        updated["created_at"], created["created_at"],
        "created_at must not move on a PUT"
    );
    assert_ne!(
        updated["updated_at"], created["updated_at"],
        "updated_at must move on a PUT"
    );
}

#[test]
#[ignore = "system test: needs the services"]
fn a_deleted_note_is_gone_after_a_restart() {
    let db_url = database_url();
    let token = "system-delete-token";

    let id = {
        let service = Service::start(&db_url, token, unique_port());
        let (status, created) = service.post_note("temporary", "gone soon");
        assert_eq!(status, 201);
        let id = created["id"]
            .as_str()
            .expect("the created note has an id")
            .to_string();

        let (status, _) = service.delete_note(&id);
        assert_eq!(status, 204);
        id
        // `service` is dropped here: the process that deleted the note is
        // killed before the next one looks for it.
    };

    let service = Service::start(&db_url, token, unique_port());
    let (status, _) = service.get_note(&id);

    assert_eq!(status, 404);
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

// The four tests below are this mission's own proof obligation (see
// MISSION.md): L1's filter and L2's window are pushed down into
// `PostgresStore`'s SQL (L3), and a system test is the only place that SQL
// runs for real rather than against `StubConnection`. Each uses its own
// `unique_marker()` as a substring nothing else in the (never reset, see
// this file's header) table is expected to contain, so a leftover note from
// an earlier run — or another test in this same run — cannot change how
// many rows a query here matches.

#[test]
#[ignore = "system test: needs the services"]
fn q_matches_the_title_case_insensitively_against_the_real_database() {
    let service = Service::start(&database_url(), "system-search-title-token", unique_port());
    let marker = unique_marker();

    let (status, matching) = service.post_note(&format!("Groceries-{marker}"), "milk, eggs");
    assert_eq!(status, 201);
    let (status, _other) = service.post_note("taxes", "file by april");
    assert_eq!(status, 201);

    // Upper-cased, against a title that carries it lower-cased: proves the
    // real `ILIKE` answers the same case-insensitivity question
    // `ListQuery::matches` does for `MemoryStore`, not just the parameter
    // `PostgresStore::list`'s unit tests already checked reaches the
    // driver.
    let query = format!(
        "q={}",
        encode_query_value(&format!("groceries-{marker}").to_uppercase())
    );
    let (status, notes) = service.list_notes_query(&query);

    assert_eq!(status, 200);
    let notes = notes.as_array().expect("a JSON array");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["id"], matching["id"]);
}

#[test]
#[ignore = "system test: needs the services"]
fn q_matches_the_body_against_the_real_database() {
    let service = Service::start(&database_url(), "system-search-body-token", unique_port());
    let marker = unique_marker();

    let (status, matching) = service.post_note("shopping", &format!("call the plumber-{marker}"));
    assert_eq!(status, 201);
    let (status, _other) = service.post_note("unrelated", "nothing about plumbing here");
    assert_eq!(status, 201);

    let query = format!("q={}", encode_query_value(&marker));
    let (status, notes) = service.list_notes_query(&query);

    assert_eq!(status, 200);
    let notes = notes.as_array().expect("a JSON array");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["id"], matching["id"]);
}

#[test]
#[ignore = "system test: needs the services"]
fn limit_and_offset_page_through_matches_in_total_order_on_the_real_database() {
    let service = Service::start(&database_url(), "system-paging-token", unique_port());
    let marker = unique_marker();

    // Created oldest to newest; `seq DESC` lists newest first, so the
    // expected order is the reverse of creation.
    let mut created = Vec::new();
    for i in 0..5 {
        let (status, note) = service.post_note(&format!("paging-{marker}-{i}"), "");
        assert_eq!(status, 201);
        created.push(note);
    }
    created.reverse();

    let query_base = format!("q={}", encode_query_value(&marker));
    let mut paged = Vec::new();
    for offset in [0, 2, 4] {
        let (status, notes) =
            service.list_notes_query(&format!("{query_base}&limit=2&offset={offset}"));
        assert_eq!(status, 200);
        paged.extend(notes.as_array().expect("a JSON array").clone());
    }

    let paged_ids: Vec<_> = paged.iter().map(|note| note["id"].clone()).collect();
    let expected_ids: Vec<_> = created.iter().map(|note| note["id"].clone()).collect();
    assert_eq!(
        paged_ids, expected_ids,
        "paging by a fixed limit over the filtered rows must reconstruct \
         the original, newest-first sequence exactly: nothing skipped, \
         nothing repeated"
    );
}

#[test]
#[ignore = "system test: needs the services"]
fn a_sql_injection_shaped_title_round_trips_through_the_real_database() {
    let service = Service::start(&database_url(), "system-injection-token", unique_port());
    let marker = unique_marker();
    // The mission's own example (MISSION.md): a title that looks like SQL
    // is still just a note, whatever carries it to the database.
    let title = format!("'; drop table notes; --{marker}");

    let (status, created) = service.post_note(&title, "still just data");
    assert_eq!(status, 201);
    assert_eq!(created["title"], title);

    let query = format!("q={}", encode_query_value(&title));
    let (status, notes) = service.list_notes_query(&query);

    assert_eq!(status, 200);
    let notes = notes.as_array().expect("a JSON array");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["id"], created["id"]);

    // The table is still there: an ordinary request right after this one
    // still works, rather than failing because `notes` is gone.
    let (status, _) = service.list_notes();
    assert_eq!(status, 200);
}
