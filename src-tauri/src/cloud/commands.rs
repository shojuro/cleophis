//! Tauri command wrappers around `cloud::session::Cloud`. Thin by design:
//! clone the `Arc<Cloud>` out of managed state, run the (synchronous,
//! blocking-network) session method on a `spawn_blocking` thread so it
//! never stalls the async runtime, then map `CloudError` to its
//! user-facing message — the front-end only ever sees a `String` error.

use std::sync::Arc;

use tauri::{AppHandle, Manager, State};

use crate::cloud::error::CloudError;
use crate::cloud::session::{Cloud, CheckoutOutcome, PortalOutcome, SessionInfo};
use crate::cloud::store::Entitlement;
use crate::cloud::strength::{self, StrengthResult};

/// Only reachable if the blocking task itself panics or the runtime is
/// shutting down — never a `CloudError`, which is mapped separately.
const JOIN_ERROR_MESSAGE: &str = "Something went wrong on this device. Please try again.";

/// Pure/synchronous (no `Cloud` state, no network) — lets the FE render a
/// live strength meter as the user types. `check_strength` (strength.rs)
/// is the same function Task 4's enrollment flow calls as the authoritative
/// gate before deriving a verifier, so the meter and the gate never
/// disagree.
#[tauri::command]
pub fn check_password_strength(password: String, email: String, nickname: String) -> StrengthResult {
    strength::check_strength(&password, &[&email, &nickname])
}

#[tauri::command]
pub async fn sign_up(
    email: String,
    password: String,
    nickname: String,
    remember: bool,
    cloud: State<'_, Arc<Cloud>>,
) -> Result<SessionInfo, String> {
    let cloud = cloud.inner().clone();
    tauri::async_runtime::spawn_blocking(move || cloud.sign_up(&email, &password, &nickname, remember))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
        .map_err(|e: CloudError| e.user_message())
}

#[tauri::command]
pub async fn sign_in(
    email: String,
    password: String,
    remember: bool,
    cloud: State<'_, Arc<Cloud>>,
) -> Result<SessionInfo, String> {
    let cloud = cloud.inner().clone();
    tauri::async_runtime::spawn_blocking(move || cloud.sign_in(&email, &password, remember))
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

/// "Remove account from this device" (Task 6) — a destructive, outward-
/// visible local action, user-initiated from the profile menu with a
/// confirm dialog (Task 7). Always deletes `user_id`'s offline-auth
/// footprint (keyring verifier + `auth-cache/<user_id>.json`) via
/// `Cloud::remove_account_auth`, which itself refuses unless `user_id` is a
/// KNOWN local account and signs it out first if it's the active session.
/// When `wipe_local_data` is set, also deletes the account's personal packs
/// (`kpack::delete_account_packs`) and its local chats
/// (`ConvStore::delete_account_conversations`) — both go through their OWN
/// module's `account_dir_segment` sanitizer, so this command never builds a
/// per-account path itself; it only delegates to the three helpers that
/// already own their respective managed directories. The auth-footprint
/// deletion runs FIRST and is required to succeed before either wipe step
/// is attempted — an unknown/malformed `user_id` is refused before this
/// command touches packs or chats at all.
#[tauri::command]
pub async fn remove_account_from_device(
    user_id: String,
    wipe_local_data: bool,
    app: AppHandle,
    cloud: State<'_, Arc<Cloud>>,
) -> Result<(), String> {
    remove_account_impl(user_id, wipe_local_data, app, cloud.inner().clone()).await
}

/// "Remove account from this device" for the CURRENTLY signed-in account —
/// the profile-menu affordance (Task 7). The user id is resolved from the
/// authoritative session (`current_user_id`), never taken from the front
/// end, so a renderer can only ever remove the account it is signed into.
#[tauri::command]
pub async fn remove_current_account_from_device(
    wipe_local_data: bool,
    app: AppHandle,
    cloud: State<'_, Arc<Cloud>>,
) -> Result<(), String> {
    let cloud = cloud.inner().clone();
    let user_id = cloud
        .current_user_id()
        .ok_or_else(|| "You're not signed in.".to_string())?;
    remove_account_impl(user_id, wipe_local_data, app, cloud).await
}

/// Shared body for the two remove-account commands (Task 6): remove the
/// auth footprint first (which gates on the account being known + signs it
/// out if active), then, only on success, optionally wipe its local data.
async fn remove_account_impl(
    user_id: String,
    wipe_local_data: bool,
    app: AppHandle,
    cloud: Arc<Cloud>,
) -> Result<(), String> {
    let auth_user_id = user_id.clone();
    tauri::async_runtime::spawn_blocking(move || cloud.remove_account_auth(&auth_user_id))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
        .map_err(|e: CloudError| e.user_message())?;

    if !wipe_local_data {
        return Ok(());
    }

    let packs_user_id = user_id.clone();
    let packs_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::kpack::delete_account_packs(&packs_app, &packs_user_id)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())??;

    tauri::async_runtime::spawn_blocking(move || {
        app.state::<crate::convstore::ConvStore>()
            .delete_account_conversations(&user_id)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
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
