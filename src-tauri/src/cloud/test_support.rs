//! Shared test-only helpers for the `cloud` module's mock-server tests.
//!
//! Lifted out of `auth.rs` (Task A4) so `rest.rs` (Task A6) can reuse the
//! same env-var mutex and TCP mock-server plumbing instead of duplicating
//! process-global state — two test threads racing on `CLEOPHIS_SUPABASE_*`
//! env vars from separate locks would be flaky.

#![cfg(test)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;

/// env vars (`CLEOPHIS_SUPABASE_URL`/`KEY`) are process-global — serialize
/// every test in the `cloud` module that touches them behind this lock.
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn set_mock_env(port: u16) {
    std::env::set_var("CLEOPHIS_SUPABASE_URL", format!("http://127.0.0.1:{port}"));
    std::env::set_var("CLEOPHIS_SUPABASE_KEY", "test-anon-key");
}

/// Bind an ephemeral port, spawn a thread that accepts exactly one
/// connection, drains the request, writes back `body` with `status_line`
/// (e.g. "200 OK"), then closes. Returns the port to point the client at.
pub(crate) fn start_mock_server(status_line: &'static str, body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            drain_request(&mut stream);
            write_response(&mut stream, status_line, body);
        }
    });
    port
}

/// Same as `start_mock_server`, but also hands back the raw request bytes
/// over a channel so the caller can assert on the request line, headers,
/// and body — used to verify `on_conflict` query params, the `Prefer`
/// header, and array request bodies.
pub(crate) fn start_capturing_mock_server(
    status_line: &'static str,
    body: &'static str,
) -> (u16, Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let request = drain_request_capturing(&mut stream);
            let _ = tx.send(request);
            write_response(&mut stream, status_line, body);
        }
    });
    (port, rx)
}

fn write_response(stream: &mut TcpStream, status_line: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Reads (and discards) whatever the client sends until it stops arriving.
pub(crate) fn drain_request(stream: &mut TcpStream) {
    let _ = drain_request_capturing(stream);
}

fn drain_request_capturing(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_millis(300)))
        .ok();
    let mut buf = [0u8; 4096];
    let mut collected = Vec::new();
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => collected.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    collected
}

/// A bound-then-dropped listener: nothing is listening on the returned
/// port, so connecting to it yields a transport error immediately.
pub(crate) fn unused_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind throwaway");
    listener.local_addr().unwrap().port()
}

/// Serves a fixed sequence of canned responses, one per successive TCP
/// connection, in order — for multi-request flows (Task A5's session
/// manager chains several HTTP calls per public method: e.g. restore's
/// refresh -> profile -> entitlements) where each call needs its own
/// distinct response. A connection beyond the end of `responses` (e.g. the
/// session manager's fire-and-forget device-upsert thread arriving after
/// the scripted flow completes) is simply not accepted; the listener
/// thread exits after serving the last one, so any late connection fails
/// fast rather than hanging.
pub(crate) fn start_mock_server_n(responses: Vec<(&'static str, &'static str)>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for (status_line, body) in responses {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    drain_request(&mut stream);
                    write_response(&mut stream, status_line, body);
                }
                Err(_) => break,
            }
        }
    });
    port
}

/// Behavior a single TCP connection should exhibit when serving the ranged
/// mock server's `content` — see `start_ranged_server`. Used by C3b's
/// `download` module tests to exercise resume, disconnects, stalls, and
/// mid-flight auth failures against a real (if tiny) HTTP server rather than
/// mocking `ureq` itself.
#[derive(Clone)]
pub(crate) enum RangedBehavior {
    /// Honors an incoming `Range: bytes=N-` header with a correct `206
    /// Partial Content` (+ `Content-Range`); with no `Range` header, serves
    /// a plain `200` with the full body.
    Serve206,
    /// Always serves `200 OK` with the full body, ignoring any `Range`
    /// header the client sent — simulates a CDN edge that doesn't support
    /// resume; the download worker must detect this and restart clean.
    ServeFullIgnoringRange,
    /// Writes headers (advertising the FULL remaining length) plus up to
    /// `n` body bytes, then closes the connection without completing the
    /// advertised `Content-Length` — simulates a network drop mid-transfer.
    DisconnectAfter(usize),
    /// Writes headers plus a few body bytes, then sleeps far longer than
    /// any sane read timeout, leaving the connection open — simulates a
    /// stalled transfer. Pair with `CLEOPHIS_DOWNLOAD_READ_TIMEOUT_MS` set
    /// small so a test doesn't actually wait out the stall.
    StallForever,
    /// Responds with `code` and an empty body — used to simulate a
    /// mid-flight auth failure (401/403) on (re)connect.
    Status(u16),
}

/// One request as observed by the ranged mock server: the `Range` and
/// `Authorization` header VALUES only (never anything else about the
/// request) — enough for tests to assert resume offsets, and to confirm the
/// B2 authorization token appears nowhere except this header.
pub(crate) struct CapturedRequest {
    pub range: Option<String>,
    pub authorization: Option<String>,
}

/// Serves `content` over successive TCP connections; connection `i`
/// (0-indexed) is handled per `script[i.min(script.len() - 1)]` — the last
/// script entry repeats for every connection beyond the script's length, so
/// e.g. `[DisconnectAfter(100_000), Serve206]` reads as "fail once, then
/// behave forever after." Every request's `Range`/`Authorization` headers
/// are sent on the returned channel before the behavior is served, so a
/// test can drain it to assert resume offsets and header shape.
///
/// The listener thread accepts connections until the socket errors, which
/// in practice only happens when the test process tears the thread down at
/// exit — tests are not expected to join the returned handle.
///
/// Each accepted connection is handled on its OWN thread rather than inline
/// in the accept loop: `RangedBehavior::StallForever` deliberately blocks
/// for several seconds inside its handler, and if that ran on the shared
/// accept-loop thread it would starve every connection queued behind it —
/// including the client's own retry, which would then spuriously time out
/// too. Since callers only ever have one request in flight at a time
/// (sequential retries), connections are still handled in acceptance order
/// in practice even though they run concurrently in principle.
pub(crate) fn start_ranged_server(
    content: Vec<u8>,
    script: Vec<RangedBehavior>,
) -> (String, std::thread::JoinHandle<()>, Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ranged mock server");
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{port}");
    let (tx, rx) = channel();

    let content = std::sync::Arc::new(content);
    let script = std::sync::Arc::new(script);

    let handle = std::thread::spawn(move || {
        let mut conn_index = 0usize;
        while let Ok((mut stream, _)) = listener.accept() {
            let idx = conn_index.min(script.len().saturating_sub(1));
            let behavior = script[idx].clone();
            conn_index += 1;

            let content = content.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let headers = read_request_headers(&mut stream);
                let range = extract_header(&headers, "range");
                let authorization = extract_header(&headers, "authorization");
                let _ = tx.send(CapturedRequest {
                    range: range.clone(),
                    authorization,
                });

                serve_ranged(&mut stream, &content, &behavior, range.as_deref());
            });
        }
    });

    (base_url, handle, rx)
}

/// Reads raw request bytes up to (and including) the blank line ending the
/// headers — GET requests carry no body, so this is the whole request.
fn read_request_headers(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    let mut buf: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                buf.push(byte[0]);
                if buf.len() >= 4 && buf[buf.len() - 4..] == *b"\r\n\r\n" {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn extract_header(headers: &str, name: &str) -> Option<String> {
    let target = name.to_ascii_lowercase();
    headers.split("\r\n").find_map(|line| {
        let (k, v) = line.split_once(':')?;
        if k.trim().to_ascii_lowercase() == target {
            Some(v.trim().to_string())
        } else {
            None
        }
    })
}

/// Parses `bytes=N-` -> `N`. Anything else (missing/malformed) -> None.
fn parse_range_start(range: &str) -> Option<usize> {
    range
        .strip_prefix("bytes=")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
}

fn serve_ranged(
    stream: &mut TcpStream,
    content: &[u8],
    behavior: &RangedBehavior,
    range: Option<&str>,
) {
    match behavior {
        RangedBehavior::Status(code) => {
            let reason = match code {
                401 => "Unauthorized",
                403 => "Forbidden",
                _ => "Error",
            };
            let head = format!(
                "HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.flush();
        }
        RangedBehavior::StallForever => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                content.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let take = content.len().min(1024);
            let _ = stream.write_all(&content[..take]);
            let _ = stream.flush();
            // Long enough that any sane (or test-overridden) read timeout
            // fires first; the connection is torn down when this function
            // returns and the caller's loop drops `stream`.
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
        RangedBehavior::DisconnectAfter(n) => {
            let start = range
                .and_then(parse_range_start)
                .unwrap_or(0)
                .min(content.len());
            let slice = &content[start..];
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                slice.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let take = (*n).min(slice.len());
            let _ = stream.write_all(&slice[..take]);
            let _ = stream.flush();
            // Deliberately stop short of `Content-Length` — dropping
            // `stream` here (the caller's loop iteration ends) closes the
            // socket, which the client observes as an early EOF.
        }
        RangedBehavior::Serve206 => match range.and_then(parse_range_start) {
            Some(start) if start < content.len() => write_206(stream, content, start),
            _ => write_200(stream, content),
        },
        RangedBehavior::ServeFullIgnoringRange => write_200(stream, content),
    }
}

/// Serves requests from a fixed `(path, body)` routing table — a
/// PATH-AWARE alternative to `start_mock_server`/`start_ranged_server`
/// (both of which serve exactly one blob regardless of the request path),
/// needed by Task B2's catalog-fetch tests: `GET /catalog.json` and `GET
/// /catalog.json.sig` must return DIFFERENT bodies from the SAME base URL.
/// A request whose path isn't in `routes` gets a plain `404`.
///
/// Accepts connections forever on its own thread (same lingering
/// background-thread shape as `start_mock_server`/`start_ranged_server` —
/// tests are not expected to join the returned handle) and handles each
/// connection inline, one at a time: unlike `start_ranged_server`, these
/// tests never need to model a stall or a slow body, so there's no need for
/// the per-connection-thread trick that exists there solely to keep
/// `RangedBehavior::StallForever` from starving other connections.
pub(crate) fn start_path_server(
    routes: Vec<(String, Vec<u8>)>,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind path-aware mock server");
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{port}");

    let handle = std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let headers = read_request_headers(&mut stream);
            let path = headers
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("")
                .to_string();

            match routes.iter().find(|(route_path, _)| *route_path == path) {
                Some((_, body)) => write_binary_response(&mut stream, "200 OK", body),
                None => write_binary_response(&mut stream, "404 Not Found", &[]),
            }
        }
    });

    (base_url, handle)
}

fn write_binary_response(stream: &mut TcpStream, status_line: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn write_200(stream: &mut TcpStream, content: &[u8]) {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        content.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(content);
    let _ = stream.flush();
}

fn write_206(stream: &mut TcpStream, content: &[u8], start: usize) {
    let total = content.len();
    let end = total.saturating_sub(1);
    let body = &content[start..];
    let head = format!(
        "HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nContent-Range: bytes {start}-{end}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}
