use crate::auth::token::{BuilderIdToken, DeviceRegistration};
use anyhow::Result;
use chrono::{DateTime, Utc};
use http_client::HttpClient;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

pub struct DeviceFlowClient {
    _http_client: Arc<dyn HttpClient>,
    _region: String,
}

impl DeviceFlowClient {
    pub fn new(http_client: Arc<dyn HttpClient>, region: &str) -> Self {
        Self {
            _http_client: http_client,
            _region: region.to_string(),
        }
    }

    pub async fn register_client(&self, _scopes: &[&str]) -> Result<DeviceRegistration> {
        todo!("Implement in task 4.1")
    }

    pub async fn start_device_authorization(
        &self,
        _registration: &DeviceRegistration,
    ) -> Result<DeviceAuthorizationResponse> {
        todo!("Implement in task 4.1")
    }

    pub async fn poll_for_token(
        &self,
        _registration: &DeviceRegistration,
        _device_code: &str,
        _interval: Duration,
        _expires_at: DateTime<Utc>,
    ) -> Result<BuilderIdToken, PollError> {
        todo!("Implement in task 4.1")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthorizationResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum PollError {
    #[error("authorization pending")]
    AuthorizationPending,
    #[error("slow down")]
    SlowDown,
    #[error("token expired")]
    ExpiredToken,
    #[error("access denied")]
    AccessDenied,
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}
