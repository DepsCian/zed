use crate::api::client::KiroClient;
use crate::types::error::{ApiError, KiroError};
use anyhow::Result;
use futures::AsyncReadExt;
use serde::{Deserialize, Serialize};

const LIST_MODELS_TARGET: &str = "AmazonCodeWhispererService.ListAvailableModels";

#[derive(Debug, Serialize)]
pub struct ListModelsRequest {
    pub origin: String,
}

impl Default for ListModelsRequest {
    fn default() -> Self {
        Self {
            origin: "KIRO_CLI".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListModelsResponse {
    pub default_model: Option<ModelInfo>,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TokenLimits {
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub model_id: String,
    pub model_name: String,
    #[serde(default = "default_rate_multiplier")]
    pub rate_multiplier: f64,
    #[serde(default)]
    pub token_limits: TokenLimits,
}

fn default_rate_multiplier() -> f64 {
    1.0
}

impl KiroClient {
    pub async fn list_available_models(&self) -> Result<ListModelsResponse, KiroError> {
        let request = ListModelsRequest::default();
        let url = format!("{}?origin=KIRO_CLI", self.endpoint());

        let body_bytes = serde_json::to_vec(&request).map_err(|e| KiroError::Other(e.into()))?;

        let http_request = http_client::Request::builder()
            .method(http_client::Method::POST)
            .uri(&url)
            .header("Content-Type", "application/x-amz-json-1.0")
            .header("x-amz-target", LIST_MODELS_TARGET)
            .header(
                "Authorization",
                format!("Bearer {}", self.token().access_token),
            )
            .body(http_client::AsyncBody::from(body_bytes))
            .map_err(|e| KiroError::Other(e.into()))?;

        let response = self.http_client().send(http_request).await?;

        if !response.status().is_success() {
            let status = response.status();
            let mut body = Vec::new();
            response.into_body().read_to_end(&mut body).await?;
            let error_text = String::from_utf8_lossy(&body).to_string();

            return Err(match status.as_u16() {
                429 => KiroError::Api(ApiError::Throttling {
                    message: error_text,
                    retry_after: None,
                }),
                400 => KiroError::Api(ApiError::Validation {
                    message: error_text,
                }),
                401 | 403 => KiroError::Api(ApiError::AccessDenied {
                    message: error_text,
                }),
                500 => KiroError::Api(ApiError::InternalServerError),
                503 => KiroError::Api(ApiError::ServiceUnavailable),
                _ => KiroError::Network(format!("HTTP {}: {}", status, error_text)),
            });
        }

        let mut body = Vec::new();
        response.into_body().read_to_end(&mut body).await?;

        let models_response: ListModelsResponse =
            serde_json::from_slice(&body).map_err(|e| KiroError::Other(e.into()))?;

        Ok(models_response)
    }
}
