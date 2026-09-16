use std::process::ExitCode;
use std::sync::Arc;

use notes_api::api::{AppState, router};
use notes_api::config::Config;
use notes_api::store::memory::MemoryStore;

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("notes-api: {err}");
            return ExitCode::FAILURE;
        }
    };

    let state = AppState {
        store: Arc::new(MemoryStore::new()),
        token: Arc::from(config.token.as_str()),
    };

    let app = router(state);

    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", config.port)).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("notes-api: could not bind 0.0.0.0:{}: {err}", config.port);
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = axum::serve(listener, app).await {
        eprintln!("notes-api: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
