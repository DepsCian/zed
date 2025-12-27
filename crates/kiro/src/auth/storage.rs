use crate::auth::token::{BuilderIdToken, DeviceRegistration};
use anyhow::Result;
use credentials_provider::CredentialsProvider;
use gpui::AsyncApp;
use std::sync::Arc;

pub const TOKEN_KEY: &str = "codewhisperer:odic:token";
pub const REGISTRATION_KEY: &str = "codewhisperer:odic:device-registration";

const CREDENTIALS_USERNAME: &str = "kiro";

pub struct TokenStorage {
    credentials_provider: Arc<dyn CredentialsProvider>,
}

impl TokenStorage {
    pub fn new(credentials_provider: Arc<dyn CredentialsProvider>) -> Self {
        Self {
            credentials_provider,
        }
    }

    pub async fn load_token(&self, cx: &AsyncApp) -> Result<Option<BuilderIdToken>> {
        let credentials = self.credentials_provider.read_credentials(TOKEN_KEY, cx).await?;

        match credentials {
            Some((_, data)) => {
                let token: BuilderIdToken = serde_json::from_slice(&data)?;
                Ok(Some(token))
            }
            None => Ok(None),
        }
    }

    pub async fn save_token(&self, token: &BuilderIdToken, cx: &AsyncApp) -> Result<()> {
        let data = serde_json::to_vec(token)?;
        self.credentials_provider
            .write_credentials(TOKEN_KEY, CREDENTIALS_USERNAME, &data, cx)
            .await
    }

    pub async fn delete_token(&self, cx: &AsyncApp) -> Result<()> {
        self.credentials_provider.delete_credentials(TOKEN_KEY, cx).await
    }

    pub async fn load_registration(&self, cx: &AsyncApp) -> Result<Option<DeviceRegistration>> {
        let credentials = self
            .credentials_provider
            .read_credentials(REGISTRATION_KEY, cx)
            .await?;

        match credentials {
            Some((_, data)) => {
                let registration: DeviceRegistration = serde_json::from_slice(&data)?;
                Ok(Some(registration))
            }
            None => Ok(None),
        }
    }

    pub async fn save_registration(&self, reg: &DeviceRegistration, cx: &AsyncApp) -> Result<()> {
        let data = serde_json::to_vec(reg)?;
        self.credentials_provider
            .write_credentials(REGISTRATION_KEY, CREDENTIALS_USERNAME, &data, cx)
            .await
    }

    pub async fn delete_registration(&self, cx: &AsyncApp) -> Result<()> {
        self.credentials_provider
            .delete_credentials(REGISTRATION_KEY, cx)
            .await
    }
}
