use chrono::Duration;
use credentials_provider::CredentialsProvider;
use gpui::{App, AppContext, AsyncApp, Entity};
use http_client::HttpClient;
use kiro::{BuilderIdToken, KiroClient, TokenStorage};
use language_model::{LanguageModel, LanguageModelProviderState, RateLimiter};
use settings::SettingsStore;
use std::sync::Arc;

use super::auth::AuthStatus;
use super::config::DEFAULT_REGION;
use super::model::KiroModel;
use super::state::{KiroModelDefinition, KiroState};

pub struct KiroLanguageModelProvider {
    pub(crate) http_client: Arc<dyn HttpClient>,
    pub(crate) credentials_provider: Arc<dyn CredentialsProvider>,
    pub(crate) state: Entity<KiroState>,
}

impl KiroLanguageModelProvider {
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        credentials_provider: Arc<dyn CredentialsProvider>,
        cx: &mut App,
    ) -> Self {
        let state = cx.new(|cx| {
            let settings_subscription = cx.observe_global::<SettingsStore>(|_this: &mut KiroState, cx| {
                cx.notify();
            });

            KiroState {
                token: None,
                registration: None,
                auth_status: AuthStatus::SignedOut,
                region: DEFAULT_REGION.to_string(),
                available_models: Vec::new(),
                default_model_id: None,
                models_loaded: false,
                _settings_subscription: settings_subscription,
            }
        });

        let provider = Self {
            http_client,
            credentials_provider,
            state,
        };

        provider.load_stored_credentials(cx);
        provider
    }

    fn load_stored_credentials(&self, cx: &mut App) {
        let storage = TokenStorage::new(self.credentials_provider.clone());
        let state = self.state.clone();
        let http_client = self.http_client.clone();

        cx.spawn(async move |cx| {
            let token = storage.load_token(&cx).await.ok().flatten();
            let registration = storage.load_registration(&cx).await.ok().flatten();

            let region = cx.update(|cx| {
                state.update(cx, |state, cx| {
                    state.set_token(token.clone());
                    state.set_registration(registration);
                    cx.notify();
                    state.region.clone()
                })
            }).ok().unwrap_or_else(|| DEFAULT_REGION.to_string());

            if let Some(token) = token {
                if !token.is_expired() {
                    Self::fetch_models_async(http_client, token, region, state, &cx).await;
                }
            }
        })
        .detach();
    }

    pub async fn fetch_models_async(
        http_client: Arc<dyn HttpClient>,
        token: BuilderIdToken,
        region: String,
        state: Entity<KiroState>,
        cx: &AsyncApp,
    ) {
        let client = KiroClient::new(http_client, token, region);

        match client.list_available_models().await {
            Ok(response) => {
                let models: Vec<KiroModelDefinition> = response
                    .models
                    .into_iter()
                    .map(KiroModelDefinition::from)
                    .collect();

                let default_id = response.default_model.map(|m| m.model_id);

                cx.update(|cx| {
                    state.update(cx, |state, cx| {
                        state.set_available_models(models, default_id);
                        cx.notify();
                    });
                }).ok();
            }
            Err(e) => {
                log::error!("Failed to fetch Kiro models: {:?}", e);
            }
        }
    }

    pub(crate) fn create_language_model(&self, model_def: &KiroModelDefinition) -> Arc<dyn LanguageModel> {
        Arc::new(KiroModel {
            model_id: model_def.id.clone(),
            model_name: model_def.name.clone(),
            max_tokens: model_def.max_tokens,
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            credentials_provider: self.credentials_provider.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }

    pub(crate) fn create_default_model(&self) -> Arc<dyn LanguageModel> {
        Arc::new(KiroModel {
            model_id: "auto".to_string(),
            model_name: "Auto".to_string(),
            max_tokens: 200000,
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            credentials_provider: self.credentials_provider.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }

    pub(crate) fn region(&self, cx: &App) -> String {
        self.state.read(cx).region.clone()
    }

    pub(crate) fn refresh_models(&self, cx: &mut App) {
        let state = self.state.clone();
        let http_client = self.http_client.clone();

        let (token, region) = {
            let s = state.read(cx);
            (s.token.clone(), s.region.clone())
        };

        if let Some(token) = token {
            if !token.is_expired() {
                cx.spawn(async move |cx| {
                    Self::fetch_models_async(http_client, token, region, state, &cx).await;
                }).detach();
            }
        }
    }

    pub async fn ensure_valid_token(
        http_client: Arc<dyn HttpClient>,
        credentials_provider: Arc<dyn CredentialsProvider>,
        state: Entity<KiroState>,
        cx: &AsyncApp,
    ) -> Option<(BuilderIdToken, String)> {
        let (token, registration, region) = cx.update(|cx| {
            let s = state.read(cx);
            (s.token.clone(), s.registration.clone(), s.region.clone())
        }).ok()?;

        let token = token?;
        let registration = registration?;

        if !token.expires_soon(Duration::minutes(5)) {
            return Some((token, region));
        }

        log::info!("Token expires soon, refreshing...");

        match token.refresh(&http_client, &registration).await {
            Ok(new_token) => {
                let storage = TokenStorage::new(credentials_provider);
                if let Err(e) = storage.save_token(&new_token, cx).await {
                    log::error!("Failed to save refreshed token: {:?}", e);
                }

                cx.update(|cx| {
                    state.update(cx, |state, cx| {
                        state.set_token(Some(new_token.clone()));
                        cx.notify();
                    });
                }).ok();

                log::info!("Token refreshed successfully");
                Some((new_token, region))
            }
            Err(e) => {
                log::error!("Token refresh failed: {:?}", e);
                if !token.is_expired() {
                    Some((token, region))
                } else {
                    cx.update(|cx| {
                        state.update(cx, |state, cx| {
                            state.set_token(None);
                            state.set_auth_status(AuthStatus::SignedOut);
                            cx.notify();
                        });
                    }).ok();
                    None
                }
            }
        }
    }
}

impl LanguageModelProviderState for KiroLanguageModelProvider {
    type ObservableEntity = KiroState;

    fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}
