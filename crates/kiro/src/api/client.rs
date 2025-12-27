use crate::api::endpoints::get_endpoint;
use crate::auth::token::BuilderIdToken;
use anyhow::Result;
use http_client::{AsyncBody, HttpClient, Method, Request};
use serde::Serialize;
use std::sync::Arc;

const CONTENT_TYPE: &str = "application/x-amz-json-1.0";

pub struct KiroClient {
    http_client: Arc<dyn HttpClient>,
    token: BuilderIdToken,
    region: String,
}

impl KiroClient {
    pub fn new(http_client: Arc<dyn HttpClient>, token: BuilderIdToken, region: String) -> Self {
        Self {
            http_client,
            token,
            region,
        }
    }

    pub fn endpoint(&self) -> &'static str {
        get_endpoint(&self.region)
    }

    pub fn http_client(&self) -> &Arc<dyn HttpClient> {
        &self.http_client
    }

    pub fn token(&self) -> &BuilderIdToken {
        &self.token
    }

    pub fn build_request<T: Serialize>(&self, target: &str, body: &T) -> Result<Request<AsyncBody>> {
        let url = self.endpoint();
        let body_bytes = serde_json::to_vec(body)?;

        let request = Request::builder()
            .method(Method::POST)
            .uri(url)
            .header("Content-Type", CONTENT_TYPE)
            .header("x-amz-target", target)
            .header("Authorization", format!("Bearer {}", self.token.access_token))
            .body(AsyncBody::from(body_bytes))?;

        Ok(request)
    }
}
