//! Tauri command wrappers around `cloud::session::Cloud`. Thin by design:
//! clone the `Arc<Cloud>` out of managed state, run the (synchronous,
//! blocking-network) session method on a `spawn_blocking` thread so it
//! never stalls the async runtime, then map `CloudError` to its
//! user-facing message — the front-end only ever sees a `String` error.

use std::sync::Arc;

use tauri::State;

use crate::cloud::error::CloudError;
use crate::cloud::session::{Cloud, CheckoutOutcome, PortalOutcome, SessionInfo};
use crate::cloud::store::Entitlement;

/// Only reachable if the blocking task itself panics or the runtime is
/// shutting down — never a `CloudError`, which is mapped separately.
const JOIN_ERROR_MESSAGE: &str = "Something went wrong on this device. Please try again.";

#[tauri::command]
pub async fn sign_up(
    email: String,
    password: String,
    nickname: String,
    cloud: State<'_, Arc<Cloud>>,
) -> Result<SessionInfo, String> {
    let cloud = cloud.inner().clone();
    tauri::async_runtime::spawn_blocking(move || cloud.sign_up(&email, &password, &nickname))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
        .map_err(|e: CloudError| e.user_message())
}

#[tauri::command]
pub async fn sign_in(
    email: String,
    password: String,
    cloud: State<'_, Arc<Cloud>>,
) -> Result<SessionInfo, String> {
    let cloud = cloud.inner().clone();
    tauri::async_runtime::spawn_blocking(move || cloud.sign_in(&email, &password))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
        .map_err(|e: CloudError| e.user_message())
}

/// Never errs in practice — `Cloud::sign_out` always succeeds (it's
/// best-effort by design); the `Result` here only exists to surface a
/// `spawn_blocking` join failure.
#[tauri::command]
pub async fn sign_out(cloud: State<'_, Arc<Cloud>>) -> Result<(), String> {
    let cloud = cloud.inner().clone();
    tauri::async_runtime::spawn_blocking(move || cloud.sign_out())
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())
}

/// Never errs in practice — `Cloud::restore` returns a `SessionInfo`
/// unconditionally (offline is a success mode); the `Result` here only
/// exists to surface a `spawn_blocking` join failure.
#[tauri::command]
pub async fn restore_session(cloud: State<'_, Arc<Cloud>>) -> Result<SessionInfo, String> {
    let cloud = cloud.inner().clone();
    tauri::async_runtime::spawn_blocking(move || cloud.restore())
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())
}

#[tauri::command]
pub async fn grant_entitlement(
    model_id: String,
    source: String,
    cloud: State<'_, Arc<Cloud>>,
) -> Result<(), String> {
    let cloud = cloud.inner().clone();
    tauri::async_runtime::spawn_blocking(move || cloud.grant(&model_id, &source))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
        .map_err(|e: CloudError| e.user_message())
}

#[tauri::command]
pub async fn list_entitlements(cloud: State<'_, Arc<Cloud>>) -> Result<Vec<Entitlement>, String> {
    let cloud = cloud.inner().clone();
    tauri::async_runtime::spawn_blocking(move || cloud.entitlements())
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
        .map_err(|e: CloudError| e.user_message())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartCheckoutResult {
    /// "opened" | "alreadyOwned"
    pub status: String,
}

/// Mints a Stripe Checkout session for `model_id` and, unless the user
/// already owns it, opens it in the system browser via the opener plugin
/// (never inside the app's own webview — a payment page has no business
/// running under this app's CSP/IPC surface).
#[tauri::command]
pub async fn start_checkout(
    model_id: String,
    app: tauri::AppHandle,
    cloud: State<'_, Arc<Cloud>>,
) -> Result<StartCheckoutResult, String> {
    let cloud = cloud.inner().clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || cloud.create_checkout(&model_id))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
        .map_err(|e: CloudError| e.user_message())?;

    match outcome {
        CheckoutOutcome::AlreadyOwned => Ok(StartCheckoutResult {
            status: "alreadyOwned".into(),
        }),
        CheckoutOutcome::Url(u) => {
            use tauri_plugin_opener::OpenerExt;
            app.opener()
                .open_url(u, None::<&str>)
                .map_err(|_| "Couldn't open your browser — please try again.".to_string())?;
            Ok(StartCheckoutResult {
                status: "opened".into(),
            })
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBillingPortalResult {
    /// "opened" | "noBillingAccount"
    pub status: String,
}

/// Mints a Stripe Billing Portal session for the current user and, unless
/// they have never subscribed, opens it in the system browser via the
/// opener plugin (never inside the app's own webview — a billing page has
/// no business running under this app's CSP/IPC surface).
#[tauri::command]
pub async fn open_billing_portal(
    app: tauri::AppHandle,
    cloud: State<'_, Arc<Cloud>>,
) -> Result<OpenBillingPortalResult, String> {
    let cloud = cloud.inner().clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || cloud.create_portal_session())
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
        .map_err(|e: CloudError| e.user_message())?;

    match outcome {
        PortalOutcome::NoBillingAccount => Ok(OpenBillingPortalResult {
            status: "noBillingAccount".into(),
        }),
        PortalOutcome::Url(u) => {
            use tauri_plugin_opener::OpenerExt;
            app.opener()
                .open_url(u, None::<&str>)
                .map_err(|_| "Couldn't open your browser — please try again.".to_string())?;
            Ok(OpenBillingPortalResult {
                status: "opened".into(),
            })
        }
    }
}
