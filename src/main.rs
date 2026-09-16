use std::process::ExitCode;
use std::sync::Arc;

use notes_api::api::{AppState, router};
use notes_api::config::Config;
use notes_api::store::Store;
use notes_api::store::memory::MemoryStore;
use notes_api::store::postgres::PostgresStore;

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("notes-api: {err}");
            return ExitCode::FAILURE;
        }
    };

    let store: Arc<dyn Store> = match &config.database_url {
        Some(database_url) => match PostgresStore::connect(database_url).await {
            Ok(store) => Arc::new(store),
            Err(err) => {
                eprintln!("notes-api: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => Arc::new(MemoryStore::new()),
    };

    let state = AppState {
        store,
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
