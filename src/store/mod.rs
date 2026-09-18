pub mod memory;
pub mod postgres;

use std::fmt;

use uuid::Uuid;

use crate::note::Note;

/// Something went wrong keeping or fetching a note. A missing note is not
/// this: the trait's methods say so through `Option`, not through `Err`.
#[derive(Debug)]
pub struct StoreError(pub String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for StoreError {}

/// Where notes live. One trait, so the API never has to know whether it is
/// talking to memory or to PostgreSQL.
pub trait Store: Send + Sync {
    fn create(&self, title: String, body: String) -> Result<Note, StoreError>;
    fn get(&self, id: Uuid) -> Result<Option<Note>, StoreError>;
    fn list(&self, query: &ListQuery) -> Result<Vec<Note>, StoreError>;
    fn update(&self, id: Uuid, title: String, body: String) -> Result<Option<Note>, StoreError>;
    fn delete(&self, id: Uuid) -> Result<bool, StoreError>;
}

/// What a listing narrows itself to. Behind the trait, so a caller — the
/// API layer today — never has to know whether a `Store` filters in Rust or
/// pushes the filter down into a query of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListQuery {
    /// A case-insensitive substring that a note's title or body must
    /// contain. `None`, or `Some(String::new())`, means every note.
    pub q: Option<String>,
}

impl ListQuery {
    /// The one definition of "matches `q`". Every `Store` implementation
    /// filters through this rather than writing its own comparison, so that
    /// what counts as a match cannot drift between a memory store and a SQL
    /// one answering the same query.
    pub fn matches(&self, note: &Note) -> bool {
        let Some(q) = self.q.as_deref().filter(|q| !q.is_empty()) else {
            return true;
        };

        let q = q.to_lowercase();
        note.title.to_lowercase().contains(&q) || note.body.to_lowercase().contains(&q)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn store_error_displays_its_message() {
        let error = StoreError("the store's lock was poisoned".to_string());

        assert_eq!(error.to_string(), "the store's lock was poisoned");
    }

    fn note(title: &str, body: &str) -> Note {
        let now = Utc::now();
        Note {
            id: Uuid::new_v4(),
            title: title.to_string(),
            body: body.to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    fn query(q: &str) -> ListQuery {
        ListQuery {
            q: Some(q.to_string()),
        }
    }

    #[test]
    fn a_query_with_no_q_matches_every_note() {
        let query = ListQuery::default();

        assert!(query.matches(&note("groceries", "milk, eggs")));
        assert!(query.matches(&note("", "")));
    }

    #[test]
    fn a_query_with_an_empty_q_matches_every_note() {
        assert!(query("").matches(&note("groceries", "milk, eggs")));
    }

    #[test]
    fn a_query_matches_a_note_whose_title_contains_q() {
        assert!(query("groc").matches(&note("groceries", "milk, eggs")));
    }

    #[test]
    fn a_query_matches_a_note_whose_body_contains_q() {
        assert!(query("eggs").matches(&note("groceries", "milk, eggs")));
    }

    #[test]
    fn a_query_does_not_match_a_note_with_q_in_neither_title_nor_body() {
        assert!(!query("bread").matches(&note("groceries", "milk, eggs")));
    }

    #[test]
    fn a_query_matches_regardless_of_case_on_either_side() {
        assert!(query("GROC").matches(&note("Groceries", "milk, eggs")));
        assert!(query("EGGS").matches(&note("groceries", "Milk, Eggs")));
    }

    #[test]
    fn a_query_matches_a_substring_not_only_a_whole_word() {
        assert!(query("roceri").matches(&note("groceries", "milk, eggs")));
    }

    #[test]
    fn a_query_treats_sql_looking_text_as_an_ordinary_substring() {
        let title = "'; drop table notes; --";
        assert!(query("drop table").matches(&note(title, "")));
        assert!(query(title).matches(&note(title, "")));
    }
}
