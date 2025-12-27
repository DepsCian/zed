use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct DeviceFlowPrompt {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum AuthStatus {
    SignedOut,
    SigningIn { prompt: DeviceFlowPrompt },
    Authenticated,
    Error(String),
}

impl Default for AuthStatus {
    fn default() -> Self {
        Self::SignedOut
    }
}
