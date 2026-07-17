//! GoTrue (Supabase Auth) client. Synchronous/blocking — callers wrap in
//! `spawn_blocking` (Task A5). Never logs secrets, tokens, or raw bodies.

use serde_json::Value;

use crate::cloud::config;
use crate::cloud::error::CloudError;

pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub user_id: String,
    pub email: String,
}

/// POST {url}/auth/v1/token?grant_type=password  body {"email","password"}
pub fn sign_in_password(email: &str, password: &str) -> Result<TokenResponse, CloudError> {
    let url = format!("{}/auth/v1/token?grant_type=password", config::supabase_url());
    let body = serde_json::json!({ "email": email, "password": password });
    request_token(&url, body, Endpoint::SignIn)
}

/// POST {url}/auth/v1/signup  body {"email","password","data":{"nickname": nickname}}
/// Confirmations OFF → session comes back (TokenResponse). Confirmations ON →
/// response has no access_token field → return Err(CloudError::EmailNotConfirmed).
pub fn sign_up(email: &str, password: &str, nickname: &str) -> Result<TokenResponse, CloudError> {
    let url = format!("{}/auth/v1/signup", config::supabase_url());
    let body = serde_json::json!({
        "email": email,
        "password": password,
        "data": { "nickname": nickname },
    });
    request_token(&url, body, Endpoint::SignUp)
}

/// POST {url}/auth/v1/token?grant_type=refresh_token  body {"refresh_token"}
pub fn refresh(refresh_token: &str) -> Result<TokenResponse, CloudError> {
    let url = format!("{}/auth/v1/token?grant_type=refresh_token", config::supabase_url());
    let body = serde_json::json!({ "refresh_token": refresh_token });
    request_token(&url, body, Endpoint::Refresh)
}

/// POST {url}/auth/v1/logout with Authorization: Bearer <access>. Best-effort:
/// all failures swallowed (returns ()).
pub fn logout(access_token: &str) {
    let url = format!("{}/auth/v1/logout", config::supabase_url());
    let key = config::supabase_key();
    let result = agent()
        .post(&url)
        .set("apikey", &key)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {access_token}"))
        .call();
    // Best-effort: nothing to do with the result either way, but binding it
    // (rather than `let _ =`) makes the "we deliberately ignore this" intent
    // clear at a glance without a clippy warning either way.
    drop(result);
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(config::CONNECT_TIMEOUT)
        .timeout(config::OVERALL_TIMEOUT)
        .build()
}

/// Which GoTrue endpoint a request is for — drives two pieces of
/// endpoint-specific error mapping: refresh's "any 4xx → SessionExpired"
/// rule, and signup's "no access_token on 2xx → EmailNotConfirmed" rule.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    SignIn,
    SignUp,
    Refresh,
}

fn request_token(url: &str, body: Value, endpoint: Endpoint) -> Result<TokenResponse, CloudError> {
    let key = config::supabase_key();
    match agent()
        .post(url)
        .set("apikey", &key)
        .set("Content-Type", "application/json")
        .send_json(body)
    {
        Ok(resp) => {
            let status = resp.status();
            let json: Value = resp
                .into_json()
                .map_err(|_| CloudError::Internal("failed to parse auth response".into()))?;
            match parse_token_response(&json) {
                Some(token) => Ok(token),
                // Only signup legitimately omits access_token on a 2xx (email
                // confirmation pending). A bare/malformed body from sign-in or
                // refresh is unexpected, not "unconfirmed" — surface it as a
                // generic API error instead of misreporting the reason.
                None if endpoint == Endpoint::SignUp => Err(CloudError::EmailNotConfirmed),
                None => Err(CloudError::Api {
                    status,
                    msg: "malformed token response".into(),
                }),
            }
        }
        Err(ureq::Error::Transport(_)) => Err(CloudError::Offline),
        Err(ureq::Error::Status(status, resp)) => {
            let body: Option<Value> = resp.into_json().ok();
            Err(map_status_error(status, body, endpoint == Endpoint::Refresh))
        }
    }
}

fn parse_token_response(body: &Value) -> Option<TokenResponse> {
    let access_token = body.get("access_token")?.as_str()?.to_string();
    let refresh_token = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let expires_in = body.get("expires_in").and_then(Value::as_i64).unwrap_or(0);
    let expires_at = body
        .get("expires_at")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| now().saturating_add(expires_in));
    let user = body.get("user");
    let user_id = user
        .and_then(|u| u.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let email = user
        .and_then(|u| u.get("email"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some(TokenResponse {
        access_token,
        refresh_token,
        expires_at,
        user_id,
        email,
    })
}

/// Best-effort extraction of (error_code, message) across both GoTrue error
/// schemas: old `{"error","error_description"}` and new
/// `{"error_code","msg"}`.
fn extract_error_code_and_msg(body: &Value) -> (String, String) {
    let code = body
        .get("error_code")
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let msg = body
        .get("msg")
        .and_then(Value::as_str)
        .or_else(|| body.get("error_description").and_then(Value::as_str))
        .or_else(|| body.get("message").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    (code, msg)
}

fn map_status_error(status: u16, body: Option<Value>, is_refresh: bool) -> CloudError {
    let (code, msg) = body
        .as_ref()
        .map(extract_error_code_and_msg)
        .unwrap_or_default();

    if status == 429 || code == "over_request_rate_limit" {
        return CloudError::RateLimited;
    }
    // Refresh is special-cased above all other error-code matching: ANY 4xx
    // on the refresh endpoint (other than 429, handled above) means the
    // session needs to be re-established, regardless of what GoTrue's body
    // claims — including invalid_grant/invalid_credentials shapes, which on
    // every other endpoint mean something more specific.
    if is_refresh && (400..500).contains(&status) {
        return CloudError::SessionExpired;
    }
    if code == "invalid_credentials"
        || code == "invalid_grant"
        || msg.contains("Invalid login credentials")
    {
        return CloudError::InvalidCredentials;
    }
    if code == "email_not_confirmed" {
        return CloudError::EmailNotConfirmed;
    }
    if code == "user_already_exists" || code == "email_exists" || msg.contains("already registered")
    {
        return CloudError::UserExists;
    }
    if code == "weak_password" || msg.contains("Password should") {
        return CloudError::WeakPassword(msg);
    }
    CloudError::Api { status, msg }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::error::CloudError;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    /// env vars (`CLEOPHIS_SUPABASE_URL`/`KEY`) are process-global — serialize
    /// every test that touches them behind this lock.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_mock_env(port: u16) {
        std::env::set_var("CLEOPHIS_SUPABASE_URL", format!("http://127.0.0.1:{port}"));
        std::env::set_var("CLEOPHIS_SUPABASE_KEY", "test-anon-key");
    }

    /// Bind an ephemeral port, spawn a thread that accepts exactly one
    /// connection, drains the request, writes back `body` with `status_line`
    /// (e.g. "200 OK"), then closes. Returns the port to point the client at.
    fn start_mock_server(status_line: &'static str, body: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                drain_request(&mut stream);
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        port
    }

    /// Reads (and discards) whatever the client sends until it stops
    /// arriving. We don't assert on the request in these tests — the mock
    /// only needs to drain it so the client doesn't see a reset.
    fn drain_request(stream: &mut std::net::TcpStream) {
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(300)))
            .ok();
        let mut buf = [0u8; 2048];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    }

    /// A bound-then-dropped listener: nothing is listening on the returned
    /// port, so connecting to it yields a transport error immediately.
    fn unused_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind throwaway");
        listener.local_addr().unwrap().port()
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    /// `TokenResponse` intentionally has no `Debug` impl (never Debug-print
    /// tokens) so `.unwrap_err()` doesn't compile — this does the same job
    /// without needing one.
    fn expect_err<T>(result: Result<T, CloudError>) -> CloudError {
        match result {
            Err(e) => e,
            Ok(_) => panic!("expected an error"),
        }
    }

    // 1. sign_in success — canned 200 with full token body incl. expires_at.
    #[test]
    fn sign_in_success_full_body() {
        let _g = lock();
        let port = start_mock_server(
            "200 OK",
            r#"{"access_token":"at1","token_type":"bearer","expires_in":3600,"expires_at":1700000000,"refresh_token":"rt1","user":{"id":"user-1","email":"a@example.com"}}"#,
        );
        set_mock_env(port);
        let resp = sign_in_password("a@example.com", "hunter2").expect("expected success");
        assert_eq!(resp.access_token, "at1");
        assert_eq!(resp.refresh_token, "rt1");
        assert_eq!(resp.expires_at, 1700000000);
        assert_eq!(resp.user_id, "user-1");
        assert_eq!(resp.email, "a@example.com");
    }

    // 2. sign_in success WITHOUT expires_at → expires_at ≈ now + expires_in.
    #[test]
    fn sign_in_success_computes_expires_at() {
        let _g = lock();
        let port = start_mock_server(
            "200 OK",
            r#"{"access_token":"at2","token_type":"bearer","expires_in":3600,"refresh_token":"rt2","user":{"id":"user-2","email":"b@example.com"}}"#,
        );
        set_mock_env(port);
        let resp = sign_in_password("b@example.com", "hunter2").expect("expected success");
        let expected = now() + 3600;
        assert!(
            (resp.expires_at - expected).abs() <= 5,
            "expires_at {} not within 5s of {}",
            resp.expires_at,
            expected
        );
    }

    // 3. sign_in 400 old-schema {"error","error_description"} → InvalidCredentials.
    #[test]
    fn sign_in_400_old_schema_invalid_credentials() {
        let _g = lock();
        let port = start_mock_server(
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"Invalid login credentials"}"#,
        );
        set_mock_env(port);
        let err = expect_err(sign_in_password("a@example.com", "wrong"));
        assert!(matches!(err, CloudError::InvalidCredentials));
    }

    // 4. sign_in 400 new-schema {"error_code","msg"} → InvalidCredentials.
    #[test]
    fn sign_in_400_new_schema_invalid_credentials() {
        let _g = lock();
        let port = start_mock_server(
            "400 Bad Request",
            r#"{"code":400,"error_code":"invalid_credentials","msg":"Invalid login credentials"}"#,
        );
        set_mock_env(port);
        let err = expect_err(sign_in_password("a@example.com", "wrong"));
        assert!(matches!(err, CloudError::InvalidCredentials));
    }

    // 5. sign_up 422 user_already_exists → UserExists.
    #[test]
    fn sign_up_422_user_already_exists() {
        let _g = lock();
        let port = start_mock_server(
            "422 Unprocessable Entity",
            r#"{"code":422,"error_code":"user_already_exists","msg":"User already registered"}"#,
        );
        set_mock_env(port);
        let err = expect_err(sign_up("a@example.com", "hunter2", "nick"));
        assert!(matches!(err, CloudError::UserExists));
    }

    // 6. sign_up 200 bare user (no access_token) → EmailNotConfirmed.
    #[test]
    fn sign_up_200_bare_user_email_not_confirmed() {
        let _g = lock();
        let port = start_mock_server(
            "200 OK",
            r#"{"id":"user-3","email":"c@example.com","aud":"authenticated"}"#,
        );
        set_mock_env(port);
        let err = expect_err(sign_up("c@example.com", "hunter2", "nick"));
        assert!(matches!(err, CloudError::EmailNotConfirmed));
    }

    // 7a. refresh 400 (unmatched code) → SessionExpired.
    #[test]
    fn refresh_400_session_expired() {
        let _g = lock();
        let port = start_mock_server(
            "400 Bad Request",
            r#"{"msg":"Invalid Refresh Token: Already Used"}"#,
        );
        set_mock_env(port);
        let err = expect_err(refresh("some-refresh-token"));
        assert!(matches!(err, CloudError::SessionExpired));
    }

    // 7a-bis. refresh 400 with an old-schema invalid_grant body still maps to
    // SessionExpired — refresh's "any 4xx → SessionExpired" rule takes
    // precedence over the credential-specific codes, which apply everywhere
    // else.
    #[test]
    fn refresh_400_invalid_grant_still_session_expired() {
        let _g = lock();
        let port = start_mock_server(
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"Invalid Refresh Token"}"#,
        );
        set_mock_env(port);
        let err = expect_err(refresh("some-refresh-token"));
        assert!(matches!(err, CloudError::SessionExpired));
    }

    // 7b. refresh 429 → RateLimited.
    #[test]
    fn refresh_429_rate_limited() {
        let _g = lock();
        let port = start_mock_server(
            "429 Too Many Requests",
            r#"{"error_code":"over_request_rate_limit","msg":"rate limited"}"#,
        );
        set_mock_env(port);
        let err = expect_err(refresh("some-refresh-token"));
        assert!(matches!(err, CloudError::RateLimited));
    }

    // 8. transport error (connect to a port with no listener) → Offline.
    #[test]
    fn transport_error_is_offline() {
        let _g = lock();
        let port = unused_port();
        set_mock_env(port);
        let err = expect_err(sign_in_password("a@example.com", "hunter2"));
        assert!(matches!(err, CloudError::Offline));
    }

    // 9. weak password 422 new-schema → WeakPassword containing server msg.
    #[test]
    fn weak_password_422() {
        let _g = lock();
        let port = start_mock_server(
            "422 Unprocessable Entity",
            r#"{"code":422,"error_code":"weak_password","msg":"Password should be at least 6 characters"}"#,
        );
        set_mock_env(port);
        let err = expect_err(sign_up("a@example.com", "123", "nick"));
        match err {
            CloudError::WeakPassword(msg) => {
                assert!(msg.contains("Password should"), "msg was: {msg}")
            }
            other => panic!("expected WeakPassword, got {other:?}"),
        }
    }

    // sign-in 200 with no access_token is malformed, not "unconfirmed" (that
    // reading only applies to signup) → Api{status: 200, msg: "malformed..."}.
    #[test]
    fn sign_in_200_malformed_body_is_api_error() {
        let _g = lock();
        let port = start_mock_server("200 OK", r#"{"id":"user-x","email":"x@example.com"}"#);
        set_mock_env(port);
        let err = expect_err(sign_in_password("x@example.com", "hunter2"));
        match err {
            CloudError::Api { status, msg } => {
                assert_eq!(status, 200);
                assert_eq!(msg, "malformed token response");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    // logout is best-effort: must not panic even against an unreachable host.
    #[test]
    fn logout_swallows_errors() {
        let _g = lock();
        let port = unused_port();
        set_mock_env(port);
        logout("some-access-token");
    }
}
