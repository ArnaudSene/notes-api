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
    fn list(&self) -> Result<Vec<Note>, StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_error_displays_its_message() {
        let error = StoreError("the store's lock was poisoned".to_string());

        assert_eq!(error.to_string(), "the store's lock was poisoned");
    }
}
