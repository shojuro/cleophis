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
