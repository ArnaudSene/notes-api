use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::store::Store;

/// What every handler shares: the store notes live in, and the token every
/// request must carry.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn Store>,
    pub token: Arc<str>,
}

/// The router this service serves: the three routes, behind the token
/// guard.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/notes", post(create_note).get(list_notes))
        .route("/notes/{id}", get(get_note))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct CreateNote {
    title: String,
    body: String,
}

async fn create_note(
    State(state): State<AppState>,
    body: Result<Json<CreateNote>, JsonRejection>,
) -> Response {
    let Ok(Json(create)) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    if create.title.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    match state.store.create(create.title, create.body) {
        Ok(note) => (StatusCode::CREATED, Json(note)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn get_note(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match state.store.get(id) {
        Ok(Some(note)) => (StatusCode::OK, Json(note)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn list_notes(State(state): State<AppState>) -> Response {
    match state.store.list() {
        Ok(notes) => (StatusCode::OK, Json(notes)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The token guard. Runs ahead of every route: a request that does not
/// carry `Authorization: Bearer <token>`, byte for byte, never reaches a
/// handler. The response never says which of the ways it was wrong.
async fn require_token(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let carries_valid_token = request
        .headers()
        .get(header::AUTHORIZATION)
        .is_some_and(|value| is_valid_bearer(value, &state.token));

    if carries_valid_token {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn is_valid_bearer(header_value: &HeaderValue, token: &str) -> bool {
    let bytes = header_value.as_bytes();
    let prefix = b"Bearer ";

    match bytes.split_at_checked(prefix.len()) {
        Some((scheme, given)) if scheme == prefix => constant_time_eq(given, token.as_bytes()),
        _ => false,
    }
}

/// Compares two byte strings without stopping at the first difference, so
/// that how long the comparison takes does not depend on where the two
/// diverge.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }

    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::memory::MemoryStore;

    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TOKEN: &str = "s3cret-token";

    fn app() -> Router {
        router(AppState {
            store: Arc::new(MemoryStore::new()),
            token: Arc::from(TOKEN),
        })
    }

    fn authed(builder: axum::http::request::Builder) -> axum::http::request::Builder {
        builder.header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn create_request(title: &str, body: &str) -> HttpRequest<Body> {
        authed(HttpRequest::builder().method("POST").uri("/notes"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({ "title": title, "body": body }).to_string(),
            ))
            .unwrap()
    }

    #[tokio::test]
    async fn post_notes_creates_a_note() {
        let response = app()
            .oneshot(create_request("groceries", "milk, eggs"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let note = body_json(response).await;
        assert_eq!(note["title"], "groceries");
        assert_eq!(note["body"], "milk, eggs");
        assert!(note["id"].is_string());
        assert!(note["created_at"].is_string());
    }

    #[tokio::test]
    async fn post_notes_rejects_an_empty_title() {
        let response = app()
            .oneshot(create_request("", "milk, eggs"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_notes_rejects_a_body_that_is_not_a_note() {
        let request = authed(HttpRequest::builder().method("POST").uri("/notes"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"title": "no body field"}"#))
            .unwrap();

        let response = app().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_notes_rejects_a_body_that_is_not_json_at_all() {
        let request = authed(HttpRequest::builder().method("POST").uri("/notes"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("not json"))
            .unwrap();

        let response = app().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_notes_id_reads_a_note_back_as_it_was_created() {
        let app = app();

        let created = app
            .clone()
            .oneshot(create_request("groceries", "milk, eggs"))
            .await
            .unwrap();
        let created = body_json(created).await;
        let id = created["id"].as_str().unwrap();

        let request = authed(
            HttpRequest::builder()
                .method("GET")
                .uri(format!("/notes/{id}")),
        )
        .body(Body::empty())
        .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let fetched = body_json(response).await;
        assert_eq!(fetched, created);
    }

    #[tokio::test]
    async fn get_notes_id_of_an_unknown_id_is_404() {
        let request = authed(
            HttpRequest::builder()
                .method("GET")
                .uri(format!("/notes/{}", Uuid::new_v4())),
        )
        .body(Body::empty())
        .unwrap();

        let response = app().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_notes_id_of_a_malformed_id_is_404() {
        let request = authed(
            HttpRequest::builder()
                .method("GET")
                .uri("/notes/not-a-uuid"),
        )
        .body(Body::empty())
        .unwrap();

        let response = app().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_notes_lists_notes_newest_first() {
        let app = app();

        app.clone()
            .oneshot(create_request("first", "1"))
            .await
            .unwrap();
        app.clone()
            .oneshot(create_request("second", "2"))
            .await
            .unwrap();

        let request = authed(HttpRequest::builder().method("GET").uri("/notes"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let notes = body_json(response).await;
        let notes = notes.as_array().unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0]["title"], "second");
        assert_eq!(notes[1]["title"], "first");
    }

    #[tokio::test]
    async fn get_notes_of_an_empty_store_is_an_empty_array() {
        let request = authed(HttpRequest::builder().method("GET").uri("/notes"))
            .body(Body::empty())
            .unwrap();

        let response = app().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let notes = body_json(response).await;
        assert_eq!(notes, serde_json::json!([]));
    }

    #[tokio::test]
    async fn no_authorization_header_is_401() {
        let request = HttpRequest::builder()
            .method("GET")
            .uri("/notes")
            .body(Body::empty())
            .unwrap();

        let response = app().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_header_that_is_not_bearer_is_401() {
        let request = HttpRequest::builder()
            .method("GET")
            .uri("/notes")
            .header(header::AUTHORIZATION, format!("Basic {TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let response = app().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_token_that_differs_is_401() {
        let request = HttpRequest::builder()
            .method("GET")
            .uri("/notes")
            .header(header::AUTHORIZATION, "Bearer wrong-token")
            .body(Body::empty())
            .unwrap();

        let response = app().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_empty_bearer_token_is_401() {
        let request = HttpRequest::builder()
            .method("GET")
            .uri("/notes")
            .header(header::AUTHORIZATION, "Bearer ")
            .body(Body::empty())
            .unwrap();

        let response = app().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_guard_runs_before_the_create_route_too() {
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/notes")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"title": "t", "body": "b"}"#))
            .unwrap();

        let response = app().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn constant_time_eq_rejects_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer-value"));
    }

    #[test]
    fn constant_time_eq_accepts_equal_bytes() {
        assert!(constant_time_eq(b"same", b"same"));
    }

    #[test]
    fn constant_time_eq_rejects_a_single_differing_byte() {
        assert!(!constant_time_eq(b"aaaa", b"aaab"));
    }
}
