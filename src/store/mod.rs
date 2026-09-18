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

/// The most a single listing will ever hand back, whatever a caller asks
/// for: the ceiling is the service's to set, not the caller's — a `limit`
/// with no cap would let a caller ask the service to build everything it
/// holds in one response. Also what a listing returns when the caller
/// gives no `limit` at all: "as many as you'd give me by default" is this
/// many, not "everything".
pub const MAX_LIMIT: u32 = 100;

/// What a listing narrows itself to. Behind the trait, so a caller — the
/// API layer today — never has to know whether a `Store` filters and windows
/// in Rust or pushes both down into a query of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListQuery {
    /// A case-insensitive substring that a note's title or body must
    /// contain. `None`, or `Some(String::new())`, means every note.
    pub q: Option<String>,
    /// The most notes to hand back. `None`, or anything above
    /// [`MAX_LIMIT`], is the same as `Some(MAX_LIMIT)`.
    pub limit: Option<u32>,
    /// How many matching notes, in order, to skip before the window
    /// starts. `None` is the same as `Some(0)`.
    pub offset: Option<u32>,
}

impl ListQuery {
    /// The one definition of "matches `q`": a case-insensitive substring
    /// over title or body. `MemoryStore` filters through this directly.
    /// `PostgresStore` expresses the same contract in SQL instead (see
    /// [`Self::like_pattern`]) — the wording of what counts as a match
    /// lives here, once, so the two cannot drift on what "matches" means
    /// even though they check it two different ways.
    pub fn matches(&self, note: &Note) -> bool {
        let Some(q) = self.q.as_deref().filter(|q| !q.is_empty()) else {
            return true;
        };

        let q = q.to_lowercase();
        note.title.to_lowercase().contains(&q) || note.body.to_lowercase().contains(&q)
    }

    /// [`Self::matches`], expressed as a SQL `LIKE`/`ILIKE` pattern instead
    /// of a Rust substring search: `q` wrapped in `%` wildcards, with any
    /// `%`, `_`, or `\` already in `q` escaped so that a literal one in a
    /// title or body still counts as a literal one — not as a wildcard
    /// `LIKE` would otherwise take it for. `None`, or an empty `q`, becomes
    /// a bare wildcard that matches every row, the same way
    /// [`Self::matches`] does.
    pub fn like_pattern(&self) -> String {
        let q = self.q.as_deref().unwrap_or("");
        let escaped = q
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");

        format!("%{escaped}%")
    }

    /// The limit this query actually applies: the caller's, clamped to
    /// [`MAX_LIMIT`], or `MAX_LIMIT` itself when the caller gave none.
    pub fn effective_limit(&self) -> u32 {
        self.limit.map_or(MAX_LIMIT, |limit| limit.min(MAX_LIMIT))
    }

    /// The offset this query actually applies: the caller's, or `0` when
    /// the caller gave none.
    pub fn effective_offset(&self) -> u32 {
        self.offset.unwrap_or(0)
    }

    /// The one definition of "the window `limit`/`offset` describe": skip
    /// [`Self::effective_offset`] notes, then take at most
    /// [`Self::effective_limit`]. `MemoryStore` calls this directly, on
    /// notes it has already filtered and ordered. `PostgresStore`
    /// expresses the same window as SQL `LIMIT`/`OFFSET` instead, built
    /// from the same `effective_limit`/`effective_offset` this uses, so
    /// this method itself goes unused there.
    pub fn window(&self, notes: Vec<Note>) -> Vec<Note> {
        notes
            .into_iter()
            .skip(self.effective_offset() as usize)
            .take(self.effective_limit() as usize)
            .collect()
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
            ..Default::default()
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

    #[test]
    fn like_pattern_wraps_q_in_wildcards() {
        assert_eq!(query("milk").like_pattern(), "%milk%");
    }

    #[test]
    fn like_pattern_matches_everything_when_there_is_no_q() {
        // "%%" and "%" are equivalent LIKE patterns; either matches every
        // row, including one with an empty title or body.
        assert_eq!(ListQuery::default().like_pattern(), "%%");
    }

    #[test]
    fn like_pattern_matches_everything_when_q_is_empty() {
        assert_eq!(query("").like_pattern(), "%%");
    }

    #[test]
    fn like_pattern_escapes_a_literal_percent() {
        assert_eq!(query("50%").like_pattern(), "%50\\%%");
    }

    #[test]
    fn like_pattern_escapes_a_literal_underscore() {
        assert_eq!(query("a_b").like_pattern(), "%a\\_b%");
    }

    #[test]
    fn like_pattern_escapes_a_literal_backslash() {
        assert_eq!(query("a\\b").like_pattern(), "%a\\\\b%");
    }

    #[test]
    fn like_pattern_keeps_sql_looking_text_as_literal_data() {
        // `LIKE`'s only special characters are `%`, `_`, and the escape
        // character; a quote or a semicolon needs no escaping to stay
        // literal here, and — because this becomes a bound parameter, never
        // text spliced into the statement — cannot act as SQL either.
        let title = "'; drop table notes; --";
        assert_eq!(query(title).like_pattern(), format!("%{title}%"));
    }

    #[test]
    fn effective_offset_is_zero_when_the_caller_gives_none() {
        assert_eq!(ListQuery::default().effective_offset(), 0);
    }

    #[test]
    fn effective_offset_is_the_caller_s_offset_when_given() {
        let query = ListQuery {
            offset: Some(7),
            ..Default::default()
        };

        assert_eq!(query.effective_offset(), 7);
    }

    /// `count` notes, titled `"note 0"` through `"note {count - 1}"`, in
    /// that order — the order a store's own logic hands `window` once
    /// filtering and ordering are already done.
    fn notes(count: u32) -> Vec<Note> {
        (0..count).map(|i| note(&format!("note {i}"), "")).collect()
    }

    fn titles(notes: &[Note]) -> Vec<&str> {
        notes.iter().map(|note| note.title.as_str()).collect()
    }

    #[test]
    fn window_with_no_limit_or_offset_returns_everything_under_the_ceiling() {
        let query = ListQuery::default();

        assert_eq!(query.window(notes(3)).len(), 3);
    }

    #[test]
    fn window_with_no_limit_is_bounded_by_max_limit() {
        let query = ListQuery::default();

        let windowed = query.window(notes(MAX_LIMIT + 1));

        assert_eq!(windowed.len(), MAX_LIMIT as usize);
    }

    #[test]
    fn window_honors_a_limit_within_the_ceiling() {
        let query = ListQuery {
            limit: Some(2),
            ..Default::default()
        };

        assert_eq!(titles(&query.window(notes(5))), vec!["note 0", "note 1"]);
    }

    #[test]
    fn window_clamps_a_limit_above_the_ceiling_down_to_it() {
        let query = ListQuery {
            limit: Some(MAX_LIMIT + 50),
            ..Default::default()
        };

        let windowed = query.window(notes(MAX_LIMIT + 1));

        assert_eq!(windowed.len(), MAX_LIMIT as usize);
    }

    #[test]
    fn window_defaults_offset_to_zero() {
        let query = ListQuery {
            limit: Some(1),
            ..Default::default()
        };

        assert_eq!(titles(&query.window(notes(3))), vec!["note 0"]);
    }

    #[test]
    fn window_skips_offset_notes_before_taking_the_limit() {
        let query = ListQuery {
            limit: Some(2),
            offset: Some(2),
            ..Default::default()
        };

        assert_eq!(titles(&query.window(notes(5))), vec!["note 2", "note 3"]);
    }

    #[test]
    fn window_with_an_offset_past_the_end_is_empty() {
        let query = ListQuery {
            offset: Some(10),
            ..Default::default()
        };

        assert_eq!(query.window(notes(3)), Vec::new());
    }

    #[test]
    fn window_with_a_limit_of_zero_is_empty() {
        let query = ListQuery {
            limit: Some(0),
            ..Default::default()
        };

        assert_eq!(query.window(notes(3)), Vec::new());
    }

    /// The order `window` pages over is total (every note has its own,
    /// distinct place), so paging over it with a fixed limit, one offset
    /// after another, has to reconstruct the original sequence exactly:
    /// nothing skipped, nothing repeated.
    #[test]
    fn paging_by_a_fixed_limit_covers_every_note_exactly_once_and_in_order() {
        let source = notes(5);

        let mut paged = Vec::new();
        for offset in [0, 2, 4] {
            let query = ListQuery {
                limit: Some(2),
                offset: Some(offset),
                ..Default::default()
            };
            paged.extend(query.window(source.clone()));
        }

        assert_eq!(paged, source);
    }
}
