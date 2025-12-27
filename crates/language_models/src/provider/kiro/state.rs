use gpui::Subscription;
use kiro::{BuilderIdToken, DeviceRegistration, ModelInfo};

use super::auth::AuthStatus;

#[derive(Debug, Clone)]
pub struct KiroModelDefinition {
    pub id: String,
    pub name: String,
    pub rate_multiplier: f64,
}

impl From<ModelInfo> for KiroModelDefinition {
    fn from(info: ModelInfo) -> Self {
        Self {
            id: info.model_id,
            name: info.model_name,
            rate_multiplier: info.rate_multiplier,
        }
    }
}

pub struct KiroState {
    pub(crate) token: Option<BuilderIdToken>,
    pub(crate) registration: Option<DeviceRegistration>,
    pub(crate) auth_status: AuthStatus,
    pub(crate) region: String,
    pub(crate) available_models: Vec<KiroModelDefinition>,
    pub(crate) default_model_id: Option<String>,
    pub(crate) models_loaded: bool,
    pub(crate) _settings_subscription: Subscription,
}

impl KiroState {
    pub fn is_authenticated(&self) -> bool {
        match &self.token {
            Some(token) => !token.is_expired(),
            None => false,
        }
    }

    pub fn set_token(&mut self, token: Option<BuilderIdToken>) {
        self.token = token;
        self.auth_status = if self.token.is_some() {
            AuthStatus::Authenticated
        } else {
            AuthStatus::SignedOut
        };
    }

    pub fn set_registration(&mut self, registration: Option<DeviceRegistration>) {
        self.registration = registration;
    }

    pub fn set_auth_status(&mut self, status: AuthStatus) {
        self.auth_status = status;
    }

    pub fn set_available_models(&mut self, models: Vec<KiroModelDefinition>, default_id: Option<String>) {
        self.available_models = models;
        self.default_model_id = default_id;
        self.models_loaded = true;
    }
}
