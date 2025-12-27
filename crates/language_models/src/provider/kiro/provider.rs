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
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }

    pub(crate) fn create_default_model(&self) -> Arc<dyn LanguageModel> {
        Arc::new(KiroModel {
            model_id: "auto".to_string(),
            model_name: "Auto".to_string(),
            state: self.state.clone(),
            http_client: self.http_client.clone(),
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
}

impl LanguageModelProviderState for KiroLanguageModelProvider {
    type ObservableEntity = KiroState;

    fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}
