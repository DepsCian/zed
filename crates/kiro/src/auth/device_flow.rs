use crate::api::endpoints::get_oidc_endpoint;
use crate::auth::token::{BuilderIdToken, DeviceRegistration};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures::AsyncReadExt;
use http_client::{AsyncBody, HttpClient, Method, Request};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

const START_URL: &str = "https://view.awsapps.com/start";
const CLIENT_NAME: &str = "Kiro CLI";
const CLIENT_TYPE: &str = "public";
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

pub struct DeviceFlowClient {
    http_client: Arc<dyn HttpClient>,
    region: String,
}

impl DeviceFlowClient {
    pub fn new(http_client: Arc<dyn HttpClient>, region: &str) -> Self {
        Self {
            http_client,
            region: region.to_string(),
        }
    }

    fn oidc_endpoint(&self) -> String {
        get_oidc_endpoint(&self.region)
    }

    pub async fn register_client(&self, scopes: &[&str]) -> Result<DeviceRegistration> {
        let url = format!("{}/client/register", self.oidc_endpoint());

        let request_body = RegisterClientRequest {
            client_name: CLIENT_NAME.to_string(),
            client_type: CLIENT_TYPE.to_string(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            grant_types: vec![DEVICE_CODE_GRANT_TYPE.to_string()],
        };

        let body = serde_json::to_vec(&request_body)?;

        let request = Request::builder()
            .method(Method::POST)
            .uri(&url)
            .header("Content-Type", "application/json")
            .body(AsyncBody::from(body))?;

        let mut response = self.http_client.send(request).await?;

        let mut body = Vec::new();
        response.body_mut().read_to_end(&mut body).await?;

        if !response.status().is_success() {
            let error_text = String::from_utf8_lossy(&body);
            return Err(anyhow!("Client registration failed: {}", error_text));
        }

        let response: RegisterClientResponse = serde_json::from_slice(&body)?;

        Ok(DeviceRegistration {
            client_id: response.client_id,
            client_secret: response.client_secret,
            client_id_issued_at: response.client_id_issued_at,
            client_secret_expires_at: response.client_secret_expires_at,
            start_url: START_URL.to_string(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
        })
    }

    pub async fn start_device_authorization(
        &self,
        registration: &DeviceRegistration,
    ) -> Result<DeviceAuthorizationResponse> {
        let url = format!("{}/device_authorization", self.oidc_endpoint());

        let request_body = StartDeviceAuthorizationRequest {
            client_id: registration.client_id.clone(),
            client_secret: registration.client_secret.clone(),
            start_url: registration.start_url.clone(),
        };

        let body = serde_json::to_vec(&request_body)?;

        let request = Request::builder()
            .method(Method::POST)
            .uri(&url)
            .header("Content-Type", "application/json")
            .body(AsyncBody::from(body))?;

        let mut response = self.http_client.send(request).await?;

        let mut body = Vec::new();
        response.body_mut().read_to_end(&mut body).await?;

        if !response.status().is_success() {
            let error_text = String::from_utf8_lossy(&body);
            return Err(anyhow!("Device authorization failed: {}", error_text));
        }

        let response: DeviceAuthorizationResponse = serde_json::from_slice(&body)?;

        Ok(response)
    }

    pub async fn poll_for_token(
        &self,
        registration: &DeviceRegistration,
        device_code: &str,
        interval: Duration,
        expires_at: DateTime<Utc>,
    ) -> Result<BuilderIdToken, PollError> {
        let mut current_interval = interval;

        loop {
            if Utc::now() >= expires_at {
                return Err(PollError::ExpiredToken);
            }

            smol::Timer::after(current_interval).await;

            match self.create_token(registration, device_code).await {
                Ok(token) => return Ok(token),
                Err(PollError::AuthorizationPending) => continue,
                Err(PollError::SlowDown) => {
                    current_interval += Duration::from_secs(5);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn create_token(
        &self,
        registration: &DeviceRegistration,
        device_code: &str,
    ) -> Result<BuilderIdToken, PollError> {
        let url = format!("{}/token", self.oidc_endpoint());

        let request_body = CreateTokenRequest {
            client_id: registration.client_id.clone(),
            client_secret: registration.client_secret.clone(),
            grant_type: DEVICE_CODE_GRANT_TYPE.to_string(),
            device_code: Some(device_code.to_string()),
            refresh_token: None,
        };

        let body = serde_json::to_vec(&request_body).map_err(|e| PollError::Other(e.into()))?;

        let request = Request::builder()
            .method(Method::POST)
            .uri(&url)
            .header("Content-Type", "application/json")
            .body(AsyncBody::from(body))
            .map_err(|e| PollError::Other(e.into()))?;

        let mut response = self
            .http_client
            .send(request)
            .await
            .map_err(|e| PollError::Other(e))?;

        let mut body = Vec::new();
        response
            .body_mut()
            .read_to_end(&mut body)
            .await
            .map_err(|e| PollError::Other(e.into()))?;

        if !response.status().is_success() {
            let error: OidcErrorResponse =
                serde_json::from_slice(&body).unwrap_or_else(|_| OidcErrorResponse {
                    error: String::from_utf8_lossy(&body).to_string(),
                    error_description: None,
                });

            return match error.error.as_str() {
                "authorization_pending" => Err(PollError::AuthorizationPending),
                "slow_down" => Err(PollError::SlowDown),
                "expired_token" => Err(PollError::ExpiredToken),
                "access_denied" => Err(PollError::AccessDenied),
                _ => Err(PollError::Other(anyhow!(
                    "Token creation failed: {} - {:?}",
                    error.error,
                    error.error_description
                ))),
            };
        }

        let token_response: CreateTokenResponse =
            serde_json::from_slice(&body).map_err(|e| PollError::Other(e.into()))?;

        let expires_at = Utc::now() + ChronoDuration::seconds(token_response.expires_in);

        Ok(BuilderIdToken {
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token.unwrap_or_default(),
            expires_at,
            region: self.region.clone(),
            start_url: registration.start_url.clone(),
            scopes: registration.scopes.clone(),
        })
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterClientRequest {
    client_name: String,
    client_type: String,
    scopes: Vec<String>,
    grant_types: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterClientResponse {
    client_id: String,
    client_secret: String,
    client_id_issued_at: i64,
    client_secret_expires_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartDeviceAuthorizationRequest {
    client_id: String,
    client_secret: String,
    start_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateTokenRequest {
    client_id: String,
    client_secret: String,
    grant_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct OidcErrorResponse {
    error: String,
    error_description: Option<String>,
}
