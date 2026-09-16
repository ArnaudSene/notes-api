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
        let now = Utc::now();
        let note = Note {
            id: Uuid::new_v4(),
            title,
            body,
            created_at: now,
            updated_at: now,
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

    fn update(&self, id: Uuid, title: String, body: String) -> Result<Option<Note>, StoreError> {
        let mut notes = self
            .notes
            .lock()
            .map_err(|_| StoreError("the store's lock was poisoned".to_string()))?;

        match notes.iter_mut().find(|note| note.id == id) {
            Some(note) => {
                note.title = title;
                note.body = body;
                note.updated_at = Utc::now();
                Ok(Some(note.clone()))
            }
            None => Ok(None),
        }
    }

    fn delete(&self, id: Uuid) -> Result<bool, StoreError> {
        let mut notes = self
            .notes
            .lock()
            .map_err(|_| StoreError("the store's lock was poisoned".to_string()))?;

        let count_before = notes.len();
        notes.retain(|note| note.id != id);

        Ok(notes.len() != count_before)
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

    #[test]
    fn create_sets_updated_at_equal_to_created_at() {
        let store = MemoryStore::new();

        let note = store
            .create("title".to_string(), "body".to_string())
            .unwrap();

        assert_eq!(note.updated_at, note.created_at);
    }

    #[test]
    fn update_changes_the_title_and_body() {
        let store = MemoryStore::new();
        let created = store
            .create("title".to_string(), "body".to_string())
            .unwrap();

        let updated = store
            .update(created.id, "new title".to_string(), "new body".to_string())
            .unwrap()
            .expect("the note exists");

        assert_eq!(updated.title, "new title");
        assert_eq!(updated.body, "new body");
    }

    #[test]
    fn update_moves_updated_at_forward_but_not_created_at() {
        let store = MemoryStore::new();
        let created = store
            .create("title".to_string(), "body".to_string())
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1));

        let updated = store
            .update(created.id, "new title".to_string(), "new body".to_string())
            .unwrap()
            .expect("the note exists");

        assert!(updated.updated_at > created.updated_at);
        assert_eq!(updated.created_at, created.created_at);
    }

    #[test]
    fn update_of_an_unknown_id_is_none_not_an_error() {
        let store = MemoryStore::new();

        let updated = store
            .update(Uuid::new_v4(), "title".to_string(), "body".to_string())
            .unwrap();

        assert_eq!(updated, None);
    }

    #[test]
    fn update_of_an_unknown_id_changes_nothing() {
        let store = MemoryStore::new();
        let created = store
            .create("title".to_string(), "body".to_string())
            .unwrap();

        store
            .update(
                Uuid::new_v4(),
                "new title".to_string(),
                "new body".to_string(),
            )
            .unwrap();

        assert_eq!(store.get(created.id).unwrap(), Some(created));
    }

    #[test]
    fn update_is_reflected_by_a_later_get() {
        let store = MemoryStore::new();
        let created = store
            .create("title".to_string(), "body".to_string())
            .unwrap();

        store
            .update(created.id, "new title".to_string(), "new body".to_string())
            .unwrap();

        let fetched = store.get(created.id).unwrap().expect("the note exists");
        assert_eq!(fetched.title, "new title");
        assert_eq!(fetched.body, "new body");
    }

    #[test]
    fn list_still_orders_notes_by_creation_and_not_by_a_later_update() {
        let store = MemoryStore::new();
        let first = store.create("first".to_string(), "1".to_string()).unwrap();
        let second = store.create("second".to_string(), "2".to_string()).unwrap();
        let third = store.create("third".to_string(), "3".to_string()).unwrap();

        // Touching the oldest note must not move it to the front of `list`:
        // the order comes from creation, not from the most recent change.
        let first = store
            .update(first.id, "first, changed".to_string(), "1!".to_string())
            .unwrap()
            .unwrap();

        let notes = store.list().unwrap();

        assert_eq!(notes, vec![third, second, first]);
    }

    #[test]
    fn delete_returns_true_once_and_false_afterwards() {
        let store = MemoryStore::new();
        let created = store
            .create("title".to_string(), "body".to_string())
            .unwrap();

        assert!(store.delete(created.id).unwrap());
        assert!(!store.delete(created.id).unwrap());
    }

    #[test]
    fn delete_of_an_unknown_id_is_false() {
        let store = MemoryStore::new();

        assert!(!store.delete(Uuid::new_v4()).unwrap());
    }

    #[test]
    fn a_deleted_note_is_no_longer_found_by_get() {
        let store = MemoryStore::new();
        let created = store
            .create("title".to_string(), "body".to_string())
            .unwrap();

        store.delete(created.id).unwrap();

        assert_eq!(store.get(created.id).unwrap(), None);
    }

    #[test]
    fn deleting_one_note_leaves_the_others_in_list() {
        let store = MemoryStore::new();
        let first = store.create("first".to_string(), "1".to_string()).unwrap();
        let second = store.create("second".to_string(), "2".to_string()).unwrap();

        store.delete(first.id).unwrap();

        assert_eq!(store.list().unwrap(), vec![second]);
    }
}
