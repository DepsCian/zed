use crate::api::endpoints::get_endpoint;
use crate::auth::token::BuilderIdToken;
use http_client::HttpClient;
use std::sync::Arc;

pub struct KiroClient {
    _http_client: Arc<dyn HttpClient>,
    _token: BuilderIdToken,
    region: String,
}

impl KiroClient {
    pub fn new(http_client: Arc<dyn HttpClient>, token: BuilderIdToken, region: String) -> Self {
        Self {
            _http_client: http_client,
            _token: token,
            region,
        }
    }

    pub fn endpoint(&self) -> &'static str {
        get_endpoint(&self.region)
    }
}
