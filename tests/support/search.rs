//! Local POST fixture for search contract tests.
#![expect(
    clippy::unwrap_used,
    reason = "test fixture failures must fail the test"
)]
use serde_json::Value;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

#[derive(Debug)]
pub(crate) struct Server {
    pub(crate) origin: String,
    pub(crate) seen: Arc<Mutex<Vec<Value>>>,
    task: JoinHandle<()>,
}

impl Server {
    pub(crate) async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::clone(&seen);
        let routes: Value = serde_json::from_str(include_str!("../fixtures/search.json")).unwrap();
        let routes = Arc::new(routes.get("routes").unwrap().clone());
        let task = tokio::spawn(async move {
            let mut calls = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break; };
                        let routes = Arc::clone(&routes);
                        let requests = Arc::clone(&requests);
                        calls.spawn(async move { let _result = serve(stream, &routes, &requests).await; });
                    }
                    _done = calls.join_next(), if !calls.is_empty() => {}
                }
            }
        });
        Self { origin, seen, task }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    mut stream: TcpStream,
    routes: &Value,
    seen: &Mutex<Vec<Value>>,
) -> std::io::Result<()> {
    let mut request = Vec::new();
    loop {
        request.push(stream.read_u8().await?);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
        if request.len() > 16_384 {
            return Ok(());
        }
    }
    let text = String::from_utf8_lossy(&request);
    let path = text.split_whitespace().nth(1).unwrap_or("/");
    let length = text
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    if length > 16384 {
        return Ok(());
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    let body: Value = serde_json::from_slice(&body).unwrap();
    seen.lock()
        .unwrap()
        .push(serde_json::json!({"path":path, "headers":text, "body":body}));
    let route = &routes[path];
    tokio::time::sleep(Duration::from_millis(
        route["delay_ms"].as_u64().unwrap_or(0),
    ))
    .await;
    let body = match route["body"].as_str() {
        Some(text) => text.as_bytes().to_vec(),
        None if route["body"].is_array() => serde_json::from_value(route["body"].clone()).unwrap(),
        None => Vec::new(),
    };
    let status = route["status"].as_u64().unwrap_or(200);
    let mut head = format!("HTTP/1.1 {status} Fixture\r\nConnection: close\r\n");
    if route["no_length"] != true {
        write!(head, "Content-Length: {}\r\n", body.len()).unwrap();
    }
    if let Some(kind) = route["type"].as_str() {
        write!(head, "Content-Type: {kind}\r\n").unwrap();
    }
    if let Some(headers) = route["headers"].as_object() {
        for (key, value) in headers {
            write!(head, "{key}: {}\r\n", value.as_str().unwrap()).unwrap();
        }
    }
    head += "\r\n";
    stream.write_all(head.as_bytes()).await?;
    tokio::time::sleep(Duration::from_millis(
        route["body_delay_ms"].as_u64().unwrap_or(0),
    ))
    .await;
    stream.write_all(&body).await
}
