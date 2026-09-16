use std::sync::Mutex;

use chrono::Utc;
use uuid::Uuid;

use crate::note::Note;
use crate::store::{Store, StoreError};

/// Keeps notes in memory, in the order they were created. Nothing here
/// survives the process — it exists for tests and for a service run without
/// `DATABASE_URL`.
#[derive(Default)]
pub struct MemoryStore {
    notes: Mutex<Vec<Note>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for MemoryStore {
    fn create(&self, title: String, body: String) -> Result<Note, StoreError> {
        let note = Note {
            id: Uuid::new_v4(),
            title,
            body,
            created_at: Utc::now(),
        };

        let mut notes = self
            .notes
            .lock()
            .map_err(|_| StoreError("the store's lock was poisoned".to_string()))?;
        notes.push(note.clone());

        Ok(note)
    }

    fn get(&self, id: Uuid) -> Result<Option<Note>, StoreError> {
        let notes = self
            .notes
            .lock()
            .map_err(|_| StoreError("the store's lock was poisoned".to_string()))?;

        Ok(notes.iter().find(|note| note.id == id).cloned())
    }

    fn list(&self) -> Result<Vec<Note>, StoreError> {
        let notes = self
            .notes
            .lock()
            .map_err(|_| StoreError("the store's lock was poisoned".to_string()))?;

        Ok(notes.iter().rev().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_returns_the_note_it_stored() {
        let store = MemoryStore::new();

        let note = store
            .create("title".to_string(), "body".to_string())
            .expect("create does not fail");

        assert_eq!(note.title, "title");
        assert_eq!(note.body, "body");
    }

    #[test]
    fn create_gives_every_note_a_distinct_id() {
        let store = MemoryStore::new();

        let first = store.create("a".to_string(), "a".to_string()).unwrap();
        let second = store.create("b".to_string(), "b".to_string()).unwrap();

        assert_ne!(first.id, second.id);
    }

    #[test]
    fn get_finds_a_note_that_was_created() {
        let store = MemoryStore::new();
        let created = store
            .create("title".to_string(), "body".to_string())
            .unwrap();

        let found = store.get(created.id).unwrap();

        assert_eq!(found, Some(created));
    }

    #[test]
    fn get_finds_the_matching_note_among_several() {
        let store = MemoryStore::new();
        let first = store.create("first".to_string(), "1".to_string()).unwrap();
        let second = store.create("second".to_string(), "2".to_string()).unwrap();

        assert_eq!(store.get(first.id).unwrap(), Some(first));
        assert_eq!(store.get(second.id).unwrap(), Some(second));
    }

    #[test]
    fn get_of_an_unknown_id_is_none_not_an_error() {
        let store = MemoryStore::new();
        store
            .create("title".to_string(), "body".to_string())
            .unwrap();

        let found = store.get(Uuid::new_v4()).unwrap();

        assert_eq!(found, None);
    }

    #[test]
    fn list_is_empty_for_a_fresh_store() {
        let store = MemoryStore::new();

        let notes = store.list().unwrap();

        assert_eq!(notes, Vec::new());
    }

    #[test]
    fn list_returns_every_created_note() {
        let store = MemoryStore::new();
        store.create("a".to_string(), "a".to_string()).unwrap();
        store.create("b".to_string(), "b".to_string()).unwrap();

        let notes = store.list().unwrap();

        assert_eq!(notes.len(), 2);
    }

    #[test]
    fn list_orders_notes_newest_first() {
        let store = MemoryStore::new();
        let first = store.create("first".to_string(), "1".to_string()).unwrap();
        let second = store.create("second".to_string(), "2".to_string()).unwrap();
        let third = store.create("third".to_string(), "3".to_string()).unwrap();

        let notes = store.list().unwrap();

        assert_eq!(notes, vec![third, second, first]);
    }
}
