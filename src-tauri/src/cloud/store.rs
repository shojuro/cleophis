use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entitlement {
    pub model_id: String,
    pub source: String,
    pub created_at: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PendingGrant {
    pub model_id: String,
    pub source: String,
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct CloudCache {
    pub user_id: String,
    pub email: String,
    pub nickname: String,
    pub entitlements: Vec<Entitlement>,
    pub pending_grants: Vec<PendingGrant>,
    pub last_online_auth: i64,
}
