use std::fmt;

#[derive(Debug)]
pub enum CloudError {
    Offline,
    InvalidCredentials,
    EmailNotConfirmed,
    UserExists,
    WeakPassword(String),
    RateLimited,
    SessionExpired,
    Api { status: u16, msg: String },
    Internal(String),
}

impl CloudError {
    /// Safe for direct display in the UI. Never includes secrets, tokens,
    /// or raw response bodies beyond the server's own validation messages.
    pub fn user_message(&self) -> String {
        match self {
            CloudError::Offline => "Can't reach the server — check your connection.".into(),
            CloudError::InvalidCredentials => "Wrong email or password.".into(),
            CloudError::EmailNotConfirmed => {
                "Please confirm your email first, then log in.".into()
            }
            CloudError::UserExists => {
                "An account with that email already exists — try logging in.".into()
            }
            CloudError::WeakPassword(msg) => format!("Password too weak: {msg}"),
            CloudError::RateLimited => "Too many attempts — wait a minute and try again.".into(),
            CloudError::SessionExpired => "Session expired — please log in again.".into(),
            CloudError::Api { status, .. } => {
                format!("Server error ({status}) — please try again.")
            }
            CloudError::Internal(_) => {
                "Something went wrong on this device. Please try again.".into()
            }
        }
    }
}

impl fmt::Display for CloudError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CloudError {}
