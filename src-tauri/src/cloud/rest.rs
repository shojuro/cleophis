//! PostgREST (Supabase) client — profile, entitlements, device upsert, and
//! the local device fingerprint. Pure stateless HTTP functions that take the
//! access token as a parameter (this lands BEFORE the session manager — A5
//! composes these). Synchronous/blocking — callers wrap in `spawn_blocking`.
//! Never logs secrets, tokens, or raw bodies.

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

use crate::cloud::config;
use crate::cloud::error::CloudError;
use crate::cloud::store::Entitlement;
use crate::hardware::HardwareInfo;

/// GET {url}/rest/v1/profiles?select=nickname
/// RLS scopes to own row; response is a JSON array — return the first row's
/// nickname, or None if the array is empty.
pub fn get_profile_nickname(access_token: &str) -> Result<Option<String>, CloudError> {
    let url = format!("{}/rest/v1/profiles?select=nickname", config::supabase_url());
    let rows: Vec<ProfileRow> = get_json(&url, access_token)?;
    Ok(rows.into_iter().next().and_then(|row| row.nickname))
}

/// GET {url}/rest/v1/entitlements?select=model_id,source,created_at,expires_at
/// Deserializes snake_case DB columns via the private `EntitlementRow` DTO,
/// then converts into the camelCase-serde `store::Entitlement` — the
/// front-end contract type, which we must not change.
pub fn list_entitlements(access_token: &str) -> Result<Vec<Entitlement>, CloudError> {
    let url = format!(
        "{}/rest/v1/entitlements?select=model_id,source,created_at,expires_at",
        config::supabase_url()
    );
    let rows: Vec<EntitlementRow> = get_json(&url, access_token)?;
    Ok(rows.into_iter().map(Entitlement::from).collect())
}

/// POST {url}/rest/v1/entitlements?on_conflict=user_id,model_id
/// Prefer: resolution=ignore-duplicates — idempotent by design, so a
/// duplicate insert is a no-op success rather than a 409 conflict.
pub fn insert_entitlement(
    access_token: &str,
    user_id: &str,
    model_id: &str,
    source: &str,
) -> Result<(), CloudError> {
    let url = format!(
        "{}/rest/v1/entitlements?on_conflict=user_id,model_id",
        config::supabase_url()
    );
    let body = serde_json::json!([{
        "user_id": user_id,
        "model_id": model_id,
        "source": source,
    }]);
    post_json(&url, access_token, body, "resolution=ignore-duplicates")
}

/// POST {url}/rest/v1/devices?on_conflict=user_id,fingerprint
/// Prefer: resolution=merge-duplicates. `last_seen` is intentionally
/// omitted — the DB default applies it on insert; on a merge (conflict) it
/// does NOT get refreshed since we never send it. Acceptable per the
/// contract brief: no extra date/time dependency for a server-formatted
/// timestamp we can't produce correctly without one.
pub fn upsert_device(
    access_token: &str,
    user_id: &str,
    hw: &HardwareInfo,
    fingerprint: &str,
) -> Result<(), CloudError> {
    let url = format!(
        "{}/rest/v1/devices?on_conflict=user_id,fingerprint",
        config::supabase_url()
    );
    let body = serde_json::json!([{
        "user_id": user_id,
        "fingerprint": fingerprint,
        "gpu": hw.gpu,
        "ram_gb": hw.ram_gb as i64,
        "vram_gb": hw.vram_gb as i64,
        "platform": hw.platform,
        "tier": hw.tier,
    }]);
    post_json(&url, access_token, body, "resolution=merge-duplicates")
}

/// Response shape of the `download-url` edge function.
///
/// No `Debug` derive (mirrors `auth::TokenResponse`'s rationale, per the C3a
/// contract brief): `authorization` is a secret-ish, short-lived bearer
/// token scoped to the CDN object and must never end up in a `{:?}`/panic
/// message.
///
/// Constructed by `mint_download_url` and consumed by
/// `cloud::download::run_download`'s `auth_provider`, which places
/// `authorization` ONLY in the request's `Authorization` header.
#[derive(Clone, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DownloadAuth {
    pub url: String,
    pub authorization: String,
    pub expires_at: String,
    pub file_bytes: u64,
}

/// POST {url}/functions/v1/download-url  body {"model_id": ...}
/// Headers: apikey + Bearer (same as other rest calls). Mints a short-lived,
/// per-file CDN authorization consumed by `cloud::session::Cloud::download_authorization`,
/// in turn called by the download worker (`cloud::download::run_download`'s
/// `auth_provider`).
pub fn mint_download_url(access_token: &str, model_id: &str) -> Result<DownloadAuth, CloudError> {
    let url = format!("{}/functions/v1/download-url", config::supabase_url());
    let body = serde_json::json!({ "model_id": model_id });
    post_json_response(&url, access_token, body)
}

/// Response shape of the `create-checkout` edge function.
///
/// No `Debug` derive (mirrors `DownloadAuth`'s rationale, per the S7
/// contract brief): `url` is a payment-page capability — a live Stripe
/// Checkout session link — and must never end up in a `{:?}`/panic
/// message.
///
/// Constructed by `create_checkout` and consumed by
/// `cloud::session::Cloud::create_checkout`, which validates the URL
/// before it is ever opened in the user's browser.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutSessionResponse {
    pub url: String,
}

/// POST {url}/functions/v1/create-checkout  body {"model_id": ...}
/// Headers: apikey + Bearer (same as other rest calls). Mints a Stripe
/// Checkout session URL, consumed by `cloud::session::Cloud::create_checkout`,
/// in turn called by the `start_checkout` Tauri command.
pub fn create_checkout(
    access_token: &str,
    model_id: &str,
) -> Result<CheckoutSessionResponse, CloudError> {
    let url = format!("{}/functions/v1/create-checkout", config::supabase_url());
    let body = serde_json::json!({ "model_id": model_id });
    post_json_response(&url, access_token, body)
}

/// Response shape of the `customer-portal` edge function.
///
/// No `Debug` derive (mirrors `CheckoutSessionResponse`'s rationale, per the
/// Sub7 contract brief): `url` is a billing-portal capability — a live
/// Stripe Billing Portal session link — and must never end up in a
/// `{:?}`/panic message.
///
/// Constructed by `create_portal_session` and consumed by
/// `cloud::session::Cloud::create_portal_session`, which validates the URL
/// before it is ever opened in the user's browser.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortalSessionResponse {
    pub url: String,
}

/// POST {url}/functions/v1/customer-portal  body {}
/// Headers: apikey + Bearer (same as other rest calls). The function reads
/// no body — it derives identity solely from the Bearer JWT. Mints a Stripe
/// Billing Portal session URL, consumed by
/// `cloud::session::Cloud::create_portal_session`, in turn called by the
/// `open_billing_portal` Tauri command.
pub fn create_portal_session(access_token: &str) -> Result<PortalSessionResponse, CloudError> {
    let url = format!("{}/functions/v1/customer-portal", config::supabase_url());
    let body = serde_json::json!({});
    post_json_response(&url, access_token, body)
}

/// Stable machine identifier: hex(SHA-256(MachineGuid)) on Windows via
/// winreg (HKLM\SOFTWARE\Microsoft\Cryptography, value MachineGuid); on
/// failure or non-Windows: hex(SHA-256(hostname + "|" + OS)), hostname from
/// COMPUTERNAME/HOSTNAME, falling back to "unknown-host".
pub fn device_fingerprint() -> String {
    #[cfg(windows)]
    {
        if let Some(guid) = windows_machine_guid() {
            return hex_sha256(guid.as_bytes());
        }
    }
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown-host".to_string());
    let input = format!("{hostname}|{}", std::env::consts::OS);
    hex_sha256(input.as_bytes())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(config::CONNECT_TIMEOUT)
        .timeout(config::OVERALL_TIMEOUT)
        .build()
}

/// snake_case DB row shape for `profiles`.
#[derive(Deserialize)]
struct ProfileRow {
    nickname: Option<String>,
}

/// snake_case DB row shape for `entitlements` — the DB contract. Converts
/// into the camelCase-serde `store::Entitlement`, which is the front-end
/// contract and must not change to match this.
#[derive(Deserialize)]
struct EntitlementRow {
    model_id: String,
    source: String,
    created_at: String,
    #[serde(default)]
    expires_at: Option<String>,
}

impl From<EntitlementRow> for Entitlement {
    fn from(row: EntitlementRow) -> Self {
        Entitlement {
            model_id: row.model_id,
            source: row.source,
            created_at: row.created_at,
            expires_at: row.expires_at,
        }
    }
}

fn get_json<T: DeserializeOwned>(url: &str, access_token: &str) -> Result<T, CloudError> {
    let key = config::supabase_key();
    match agent()
        .get(url)
        .set("apikey", &key)
        .set("Authorization", &format!("Bearer {access_token}"))
        .call()
    {
        Ok(resp) => resp
            .into_json::<T>()
            .map_err(|_| CloudError::Internal("failed to parse response".into())),
        Err(ureq::Error::Transport(_)) => Err(CloudError::Offline),
        Err(ureq::Error::Status(status, resp)) => Err(map_status_error(status, resp)),
    }
}

fn post_json(
    url: &str,
    access_token: &str,
    body: Value,
    prefer: &str,
) -> Result<(), CloudError> {
    let key = config::supabase_key();
    match agent()
        .post(url)
        .set("apikey", &key)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Prefer", prefer)
        .send_json(body)
    {
        // Success = any 2xx (ureq only treats non-2xx as Err).
        Ok(_) => Ok(()),
        Err(ureq::Error::Transport(_)) => Err(CloudError::Offline),
        Err(ureq::Error::Status(status, resp)) => Err(map_status_error(status, resp)),
    }
}

/// Same agent/headers/error mapping as `post_json`, but for endpoints (edge
/// functions) that return a body on success — parses the 2xx response as
/// `T`. A malformed 2xx body (missing/mistyped fields) is reported as
/// `CloudError::Api { status: 200, msg: "malformed response" }` rather than
/// `Internal`, since it's the server's contract that broke, not something
/// local.
///
/// Only caller today is `mint_download_url`.
fn post_json_response<T: DeserializeOwned>(
    url: &str,
    access_token: &str,
    body: Value,
) -> Result<T, CloudError> {
    let key = config::supabase_key();
    match agent()
        .post(url)
        .set("apikey", &key)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {access_token}"))
        .send_json(body)
    {
        Ok(resp) => resp.into_json::<T>().map_err(|_| CloudError::Api {
            status: 200,
            msg: "malformed response".into(),
        }),
        Err(ureq::Error::Transport(_)) => Err(CloudError::Offline),
        Err(ureq::Error::Status(status, resp)) => Err(map_status_error(status, resp)),
    }
}

/// HTTP 401 → SessionExpired (the session layer's refresh-retry signal).
/// Other non-2xx → Api{status, msg}, msg extracted best-effort from the
/// PostgREST error body's "message" field — never echoes the token, which
/// never appears in a response body anyway.
fn map_status_error(status: u16, resp: ureq::Response) -> CloudError {
    if status == 401 {
        return CloudError::SessionExpired;
    }
    let body: Option<Value> = resp.into_json().ok();
    let msg = body
        .as_ref()
        .and_then(|b| b.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("request failed")
        .to_string();
    CloudError::Api { status, msg }
}

#[cfg(windows)]
fn windows_machine_guid() -> Option<String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey("SOFTWARE\\Microsoft\\Cryptography").ok()?;
    key.get_value("MachineGuid").ok()
}

fn hex_sha256(input: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::test_support;

    // 1. list_entitlements 200 with two rows (snake_case body) → Vec of
    // camelCase-typed Entitlements with exact fields.
    #[test]
    fn list_entitlements_two_rows() {
        let _g = test_support::lock();
        let port = test_support::start_mock_server(
            "200 OK",
            r#"[{"model_id":"a","source":"trial","created_at":"2026-01-01T00:00:00Z","expires_at":null},{"model_id":"b","source":"purchase","created_at":"2026-02-01T00:00:00Z","expires_at":"2026-03-01T00:00:00Z"}]"#,
        );
        test_support::set_mock_env(port);
        let result = list_entitlements("test-access-token").expect("expected success");
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            Entitlement {
                model_id: "a".into(),
                source: "trial".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                expires_at: None,
            }
        );
        assert_eq!(
            result[1],
            Entitlement {
                model_id: "b".into(),
                source: "purchase".into(),
                created_at: "2026-02-01T00:00:00Z".into(),
                expires_at: Some("2026-03-01T00:00:00Z".into()),
            }
        );
    }

    // 2. list_entitlements 401 → SessionExpired.
    #[test]
    fn list_entitlements_401_session_expired() {
        let _g = test_support::lock();
        let port = test_support::start_mock_server("401 Unauthorized", r#"{"message":"JWT expired"}"#);
        test_support::set_mock_env(port);
        let err = list_entitlements("expired-token").unwrap_err();
        assert!(matches!(err, CloudError::SessionExpired));
    }

    // 3. insert_entitlement: mock captures the request — path contains
    // on_conflict=user_id,model_id, header Prefer: resolution=ignore-duplicates
    // present, body is a one-element array with the three fields; 201 → Ok(()).
    #[test]
    fn insert_entitlement_request_shape() {
        let _g = test_support::lock();
        let (port, rx) = test_support::start_capturing_mock_server("201 Created", "[]");
        test_support::set_mock_env(port);

        let result = insert_entitlement("token", "user-1", "llama-8b", "trial");
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");

        let raw = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("expected a captured request");
        let text = String::from_utf8_lossy(&raw);
        let (head, body) = text
            .split_once("\r\n\r\n")
            .expect("expected header/body split");
        assert!(
            head.contains("on_conflict=user_id,model_id"),
            "head was: {head}"
        );
        assert!(
            head.contains("Prefer: resolution=ignore-duplicates"),
            "head was: {head}"
        );

        let parsed: Value = serde_json::from_str(body).expect("expected JSON body");
        let arr = parsed.as_array().expect("expected array body");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["user_id"], "user-1");
        assert_eq!(arr[0]["model_id"], "llama-8b");
        assert_eq!(arr[0]["source"], "trial");
    }

    // 4. insert_entitlement 409 → Api{409,..} (would only happen without the
    // Prefer header — regression canary).
    #[test]
    fn insert_entitlement_409_is_api_error() {
        let _g = test_support::lock();
        let port = test_support::start_mock_server(
            "409 Conflict",
            r#"{"code":"23505","message":"duplicate key value violates unique constraint"}"#,
        );
        test_support::set_mock_env(port);
        let err = insert_entitlement("token", "user-1", "llama-8b", "trial").unwrap_err();
        match err {
            CloudError::Api { status, .. } => assert_eq!(status, 409),
            other => panic!("expected Api, got {other:?}"),
        }
    }

    // 5. upsert_device: captures request — path contains
    // on_conflict=user_id,fingerprint, header Prefer: resolution=merge-duplicates,
    // body array has user_id/fingerprint/gpu/ram_gb/tier; 201 → Ok(()).
    #[test]
    fn upsert_device_request_shape() {
        let _g = test_support::lock();
        let (port, rx) = test_support::start_capturing_mock_server("201 Created", "[]");
        test_support::set_mock_env(port);

        let hw = HardwareInfo {
            ram_gb: 16,
            gpu: "GTX 1650".into(),
            vram_gb: 4,
            platform: "windows".into(),
            tier: "mid".into(),
        };
        let result = upsert_device("token", "user-1", &hw, "abc123fingerprint");
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");

        let raw = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("expected a captured request");
        let text = String::from_utf8_lossy(&raw);
        let (head, body) = text
            .split_once("\r\n\r\n")
            .expect("expected header/body split");
        assert!(
            head.contains("on_conflict=user_id,fingerprint"),
            "head was: {head}"
        );
        assert!(
            head.contains("Prefer: resolution=merge-duplicates"),
            "head was: {head}"
        );

        let parsed: Value = serde_json::from_str(body).expect("expected JSON body");
        let arr = parsed.as_array().expect("expected array body");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["user_id"], "user-1");
        assert_eq!(arr[0]["fingerprint"], "abc123fingerprint");
        assert_eq!(arr[0]["gpu"], "GTX 1650");
        assert_eq!(arr[0]["ram_gb"], 16);
        assert_eq!(arr[0]["tier"], "mid");
    }

    // 6. get_profile_nickname 200 [{"nickname":"jo"}] → Some("jo");
    // 200 [] → None.
    #[test]
    fn get_profile_nickname_present() {
        let _g = test_support::lock();
        let port = test_support::start_mock_server("200 OK", r#"[{"nickname":"jo"}]"#);
        test_support::set_mock_env(port);
        assert_eq!(
            get_profile_nickname("token").expect("expected success"),
            Some("jo".to_string())
        );
    }

    #[test]
    fn get_profile_nickname_empty_array_is_none() {
        let _g = test_support::lock();
        let port = test_support::start_mock_server("200 OK", "[]");
        test_support::set_mock_env(port);
        assert_eq!(get_profile_nickname("token").expect("expected success"), None);
    }

    // 7. transport error → Offline.
    #[test]
    fn transport_error_is_offline() {
        let _g = test_support::lock();
        let port = test_support::unused_port();
        test_support::set_mock_env(port);
        let err = list_entitlements("token").unwrap_err();
        assert!(matches!(err, CloudError::Offline));
    }

    // 9. mint_download_url happy path: canned 200 → struct fields exact.
    // No Debug on DownloadAuth (see its doc comment), so fields are checked
    // individually rather than via a single whole-struct assert_eq!.
    #[test]
    fn mint_download_url_happy_path() {
        let _g = test_support::lock();
        let port = test_support::start_mock_server(
            "200 OK",
            r#"{"url":"http://x/file/b/m.gguf","authorization":"tok123","expiresAt":"2026-07-18T00:00:00Z","fileBytes":2019377696}"#,
        );
        test_support::set_mock_env(port);
        let result = mint_download_url("test-access-token", "socratic-tutor").expect("expected success");
        assert_eq!(result.url, "http://x/file/b/m.gguf");
        assert_eq!(result.authorization, "tok123");
        assert_eq!(result.expires_at, "2026-07-18T00:00:00Z");
        assert_eq!(result.file_bytes, 2019377696);
    }

    // 10. mint_download_url 403 → Api{403,..} with the message preserved.
    // map_status_error extracts the "message" field from the body, so the
    // canned body uses that shape (not GoTrue's "msg"/"error_description").
    #[test]
    fn mint_download_url_403_preserves_message() {
        let _g = test_support::lock();
        let port = test_support::start_mock_server(
            "403 Forbidden",
            r#"{"message":"You don't own this model."}"#,
        );
        test_support::set_mock_env(port);
        // `.err()` rather than `.unwrap_err()`: the latter requires the Ok
        // side (DownloadAuth) to be Debug, which it deliberately isn't.
        let err = mint_download_url("token", "socratic-tutor")
            .err()
            .expect("expected an error");
        match err {
            CloudError::Api { status, msg } => {
                assert_eq!(status, 403);
                assert_eq!(msg, "You don't own this model.");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    // 11. mint_download_url 401 → SessionExpired.
    #[test]
    fn mint_download_url_401_session_expired() {
        let _g = test_support::lock();
        let port = test_support::start_mock_server("401 Unauthorized", r#"{"message":"JWT expired"}"#);
        test_support::set_mock_env(port);
        let err = mint_download_url("expired-token", "socratic-tutor")
            .err()
            .expect("expected an error");
        assert!(matches!(err, CloudError::SessionExpired));
    }

    // 12. mint_download_url malformed 200 (missing fields) →
    // Api{status:200, msg:"malformed response"}.
    #[test]
    fn mint_download_url_malformed_200_is_api_error() {
        let _g = test_support::lock();
        let port = test_support::start_mock_server("200 OK", r#"{"url":"http://x/file/b/m.gguf"}"#);
        test_support::set_mock_env(port);
        let err = mint_download_url("token", "socratic-tutor")
            .err()
            .expect("expected an error");
        match err {
            CloudError::Api { status, msg } => {
                assert_eq!(status, 200);
                assert_eq!(msg, "malformed response");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    // 13. mint_download_url request shape: path is
    // /functions/v1/download-url, body {"model_id":"socratic-tutor"}, both
    // auth headers present.
    #[test]
    fn mint_download_url_request_shape() {
        let _g = test_support::lock();
        let (port, rx) = test_support::start_capturing_mock_server(
            "200 OK",
            r#"{"url":"http://x/file/b/m.gguf","authorization":"tok123","expiresAt":"2026-07-18T00:00:00Z","fileBytes":1}"#,
        );
        test_support::set_mock_env(port);

        let result = mint_download_url("test-access-token", "socratic-tutor");
        // Not `{result:?}` — DownloadAuth deliberately has no Debug.
        if let Err(e) = &result {
            panic!("expected Ok(..), got error: {e:?}");
        }

        let raw = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("expected a captured request");
        let text = String::from_utf8_lossy(&raw);
        let (head, body) = text
            .split_once("\r\n\r\n")
            .expect("expected header/body split");
        assert!(
            head.contains("POST /functions/v1/download-url"),
            "head was: {head}"
        );
        assert!(
            head.contains("apikey: test-anon-key"),
            "head was: {head}"
        );
        assert!(
            head.contains("Authorization: Bearer test-access-token"),
            "head was: {head}"
        );

        let parsed: Value = serde_json::from_str(body).expect("expected JSON body");
        assert_eq!(parsed, serde_json::json!({"model_id": "socratic-tutor"}));
    }

    // S7-1. create_checkout request shape: path is
    // /functions/v1/create-checkout, body {"model_id":"socratic-tutor"},
    // both auth headers present.
    #[test]
    fn create_checkout_request_shape() {
        let _g = test_support::lock();
        let (port, rx) = test_support::start_capturing_mock_server(
            "200 OK",
            r#"{"url":"https://checkout.stripe.com/c/pay/cs_test_x"}"#,
        );
        test_support::set_mock_env(port);

        let result = create_checkout("test-access-token", "socratic-tutor");
        assert!(result.is_ok(), "expected Ok(..), got error");

        let raw = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("expected a captured request");
        let text = String::from_utf8_lossy(&raw);
        let (head, body) = text
            .split_once("\r\n\r\n")
            .expect("expected header/body split");
        assert!(
            head.contains("POST /functions/v1/create-checkout"),
            "head was: {head}"
        );
        assert!(
            head.contains("apikey: test-anon-key"),
            "head was: {head}"
        );
        assert!(
            head.contains("Authorization: Bearer test-access-token"),
            "head was: {head}"
        );

        let parsed: Value = serde_json::from_str(body).expect("expected JSON body");
        assert_eq!(parsed, serde_json::json!({"model_id": "socratic-tutor"}));
    }

    // S7-5. create_checkout malformed 200 (no url field) ->
    // Api{status:200, msg:"malformed response"} (from post_json_response).
    #[test]
    fn create_checkout_malformed_200_is_api_error() {
        let _g = test_support::lock();
        let port = test_support::start_mock_server("200 OK", r#"{}"#);
        test_support::set_mock_env(port);
        let err = create_checkout("token", "socratic-tutor")
            .err()
            .expect("expected an error");
        match err {
            CloudError::Api { status, msg } => {
                assert_eq!(status, 200);
                assert_eq!(msg, "malformed response");
            }
            other => panic!("expected Api, got {other:?}"),
        }
    }

    // Sub7-1. create_portal_session request shape: path is
    // /functions/v1/customer-portal, body {}, both auth headers present.
    #[test]
    fn create_portal_session_request_shape() {
        let _g = test_support::lock();
        let (port, rx) = test_support::start_capturing_mock_server(
            "200 OK",
            r#"{"url":"https://billing.stripe.com/p/session/x"}"#,
        );
        test_support::set_mock_env(port);

        let result = create_portal_session("test-access-token");
        assert!(result.is_ok(), "expected Ok(..), got error");

        let raw = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("expected a captured request");
        let text = String::from_utf8_lossy(&raw);
        let (head, body) = text
            .split_once("\r\n\r\n")
            .expect("expected header/body split");
        assert!(
            head.contains("POST /functions/v1/customer-portal"),
            "head was: {head}"
        );
        assert!(
            head.contains("apikey: test-anon-key"),
            "head was: {head}"
        );
        assert!(
            head.contains("Authorization: Bearer test-access-token"),
            "head was: {head}"
        );

        let parsed: Value = serde_json::from_str(body).expect("expected JSON body");
        assert_eq!(parsed, serde_json::json!({}));
    }

    // 8. device_fingerprint(): returns 64 lowercase hex chars; stable across
    // two calls.
    #[test]
    fn device_fingerprint_is_stable_hex() {
        let a = device_fingerprint();
        let b = device_fingerprint();
        assert_eq!(a, b, "expected device_fingerprint() to be stable");
        assert_eq!(a.len(), 64, "fingerprint was: {a}");
        assert!(
            a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "fingerprint was not lowercase hex: {a}"
        );
    }
}
