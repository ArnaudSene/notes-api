use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

static NEXT_PORT: AtomicU16 = AtomicU16::new(21001);

/// A fresh port for every service a test starts, so that several of them —
/// several system tests start the real binary as its own process — never
/// fight over the same one, whatever order or parallelism `cargo test`
/// picks.
pub fn unique_port() -> u16 {
    NEXT_PORT.fetch_add(1, Ordering::Relaxed)
}

/// The database `compose.yaml` lifts beside this container, under the name
/// the mission gives it, with the credentials that file carries. A test can
/// override it; none needs to.
pub fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://notes:notes-for-tests@db:5432/notes".to_string())
}

/// The compiled service, running as its own process against the real
/// database — the binary a deploy would run, not the router under test.
/// Killed when dropped, so a test that fails early never leaves one bound
/// to its port.
pub struct Service {
    child: Child,
    port: u16,
    token: String,
}

impl Service {
    pub fn start(database_url: &str, token: &str, port: u16) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_notes-api"))
            .env("NOTES_TOKEN", token)
            .env("DATABASE_URL", database_url)
            .env("PORT", port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start the notes-api binary");

        let service = Service {
            child,
            port,
            token: token.to_string(),
        };
        service.wait_until_serving();
        service
    }

    fn wait_until_serving(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!(
            "notes-api never answered on 127.0.0.1:{} within 15s",
            self.port
        );
    }

    pub fn post_note(&self, title: &str, body: &str) -> (u16, serde_json::Value) {
        let payload = serde_json::json!({ "title": title, "body": body }).to_string();
        self.request("POST", "/notes", Some(&payload))
    }

    pub fn get_note(&self, id: &str) -> (u16, serde_json::Value) {
        self.request("GET", &format!("/notes/{id}"), None)
    }

    pub fn list_notes(&self) -> (u16, serde_json::Value) {
        self.request("GET", "/notes", None)
    }

    pub fn put_note(&self, id: &str, title: &str, body: &str) -> (u16, serde_json::Value) {
        let payload = serde_json::json!({ "title": title, "body": body }).to_string();
        self.request("PUT", &format!("/notes/{id}"), Some(&payload))
    }

    pub fn delete_note(&self, id: &str) -> (u16, serde_json::Value) {
        self.request("DELETE", &format!("/notes/{id}"), None)
    }

    /// A hand-rolled HTTP/1.1 client: this suite's only need for one is
    /// three requests against a process it spawned itself, and that does
    /// not earn a dependency the shipped service never needed.
    fn request(&self, method: &str, path: &str, body: Option<&str>) -> (u16, serde_json::Value) {
        let mut stream =
            TcpStream::connect(("127.0.0.1", self.port)).expect("connect to notes-api");

        let body = body.unwrap_or("");
        let request = format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Authorization: Bearer {token}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {len}\r\n\
             Connection: close\r\n\r\n\
             {body}",
            token = self.token,
            len = body.len(),
        );

        stream
            .write_all(request.as_bytes())
            .expect("send the request");

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).expect("read the response");

        let response = String::from_utf8_lossy(&raw);
        let mut parts = response.splitn(2, "\r\n\r\n");
        let head = parts.next().unwrap_or_default();
        let response_body = parts.next().unwrap_or_default();

        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .expect("a status line");

        let json = if response_body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(response_body).expect("a JSON body")
        };

        (status, json)
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
