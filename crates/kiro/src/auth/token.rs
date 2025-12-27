use crate::api::endpoints::get_oidc_endpoint;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use futures::AsyncReadExt;
use http_client::{AsyncBody, HttpClient, Method, Request};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const REFRESH_TOKEN_GRANT_TYPE: &str = "refresh_token";

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

    pub async fn refresh(
        &self,
        http_client: &Arc<dyn HttpClient>,
        registration: &DeviceRegistration,
    ) -> Result<BuilderIdToken> {
        let url = format!("{}/token", get_oidc_endpoint(&self.region));

        let request_body = RefreshTokenRequest {
            client_id: registration.client_id.clone(),
            client_secret: registration.client_secret.clone(),
            grant_type: REFRESH_TOKEN_GRANT_TYPE.to_string(),
            refresh_token: self.refresh_token.clone(),
        };

        let body = serde_json::to_vec(&request_body)?;

        let request = Request::builder()
            .method(Method::POST)
            .uri(&url)
            .header("Content-Type", "application/json")
            .body(AsyncBody::from(body))?;

        let mut response = http_client.send(request).await?;

        let mut body = Vec::new();
        response.body_mut().read_to_end(&mut body).await?;

        if !response.status().is_success() {
            let error_text = String::from_utf8_lossy(&body);
            return Err(anyhow!("Token refresh failed: {}", error_text));
        }

        let token_response: RefreshTokenResponse = serde_json::from_slice(&body)?;

        let expires_at = Utc::now() + Duration::seconds(token_response.expires_in);

        Ok(BuilderIdToken {
            access_token: token_response.access_token,
            refresh_token: token_response
                .refresh_token
                .unwrap_or_else(|| self.refresh_token.clone()),
            expires_at,
            region: self.region.clone(),
            start_url: self.start_url.clone(),
            scopes: self.scopes.clone(),
        })
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshTokenRequest {
    client_id: String,
    client_secret: String,
    grant_type: String,
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}
