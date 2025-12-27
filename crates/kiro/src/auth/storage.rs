use crate::auth::token::{BuilderIdToken, DeviceRegistration};
use anyhow::Result;

pub const TOKEN_KEY: &str = "codewhisperer:odic:token";
pub const REGISTRATION_KEY: &str = "codewhisperer:odic:device-registration";

pub struct TokenStorage;

impl TokenStorage {
    pub fn new() -> Self {
        Self
    }

    pub async fn load_token(&self) -> Result<Option<BuilderIdToken>> {
        todo!("Implement in task 3.1")
    }

    pub async fn save_token(&self, _token: &BuilderIdToken) -> Result<()> {
        todo!("Implement in task 3.1")
    }

    pub async fn delete_token(&self) -> Result<()> {
        todo!("Implement in task 3.1")
    }

    pub async fn load_registration(&self) -> Result<Option<DeviceRegistration>> {
        todo!("Implement in task 3.1")
    }

    pub async fn save_registration(&self, _reg: &DeviceRegistration) -> Result<()> {
        todo!("Implement in task 3.1")
    }

    pub async fn delete_registration(&self) -> Result<()> {
        todo!("Implement in task 3.1")
    }
}

impl Default for TokenStorage {
    fn default() -> Self {
        Self::new()
    }
}
