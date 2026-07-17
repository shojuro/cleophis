use serde::Serialize;

use crate::cloud::store::Entitlement;

/// In-memory only — never serialized to disk as a whole. The refresh token
/// is mirrored to the OS keyring; the access token lives nowhere else.
#[derive(Clone, Debug)]
pub struct Session {
    pub user_id: String,
    pub email: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub signed_in: bool,
    pub nickname: Option<String>,
    pub email: Option<String>,
    /// "online" | "offlineCached" | "signedOut"
    pub mode: String,
    pub entitlements: Vec<Entitlement>,
    pub grace_expired: bool,
}

impl SessionInfo {
    pub fn signed_out() -> Self {
        SessionInfo {
            signed_in: false,
            nickname: None,
            email: None,
            mode: "signedOut".into(),
            entitlements: vec![],
            grace_expired: false,
        }
    }
}
