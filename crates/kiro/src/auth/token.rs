use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuilderIdToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub region: String,
    pub start_url: String,
    pub scopes: Vec<String>,
}

impl BuilderIdToken {
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }

    pub fn expires_soon(&self, buffer: Duration) -> bool {
        Utc::now() + buffer >= self.expires_at
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRegistration {
    pub client_id: String,
    pub client_secret: String,
    pub client_id_issued_at: i64,
    pub client_secret_expires_at: i64,
    pub start_url: String,
    pub scopes: Vec<String>,
}
