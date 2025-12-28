use anyhow::{Result, anyhow};
use chrono::Utc;
use gpui::{AnyView, App, Task};
use kiro::{DeviceFlowClient, TokenStorage};
use language_model::{
    AuthenticateError, ConfigurationViewTargetAgent, IconOrSvg, LanguageModel,
    LanguageModelProvider, LanguageModelProviderId, LanguageModelProviderName,
};
use std::sync::Arc;
use std::time::Duration;
use ui::prelude::*;

use super::auth::{AuthStatus, DeviceFlowPrompt};
use super::config::{PROVIDER_ID, PROVIDER_NAME, SCOPES};
use super::configuration_view::KiroConfigurationView;
use super::provider::KiroLanguageModelProvider;

impl LanguageModelProvider for KiroLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        PROVIDER_ID
    }

    fn name(&self) -> LanguageModelProviderName {
        PROVIDER_NAME
    }

    fn icon(&self) -> IconOrSvg {
        IconOrSvg::Icon(IconName::AiKiro)
    }

    fn default_model(&self, cx: &App) -> Option<Arc<dyn LanguageModel>> {
        let state = self.state.read(cx);

        if let Some(default_id) = &state.default_model_id {
            if let Some(model_def) = state.available_models.iter().find(|m| &m.id == default_id) {
                return Some(self.create_language_model(model_def));
            }
        }

        Some(self.create_default_model())
    }

    fn default_fast_model(&self, cx: &App) -> Option<Arc<dyn LanguageModel>> {
        let state = self.state.read(cx);

        let fast_model = state
            .available_models
            .iter()
            .find(|m| m.id.contains("haiku") || m.id == "auto");

        if let Some(model_def) = fast_model {
            return Some(self.create_language_model(model_def));
        }

        Some(self.create_default_model())
    }

    fn provided_models(&self, cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        let state = self.state.read(cx);

        if state.available_models.is_empty() {
            return vec![self.create_default_model()];
        }

        state
            .available_models
            .iter()
            .map(|model_def| self.create_language_model(model_def))
            .collect()
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        if self.is_authenticated(cx) {
            self.refresh_models(cx);
            return Task::ready(Ok(()));
        }

        let http_client = self.http_client.clone();
        let credentials_provider = self.credentials_provider.clone();
        let state = self.state.clone();
        let region = self.region(cx);

        cx.spawn(async move |cx| {
            let storage = TokenStorage::new(credentials_provider);
            let device_flow = DeviceFlowClient::new(http_client.clone(), &region);

            let registration = match storage.load_registration(&cx).await? {
                Some(reg) => reg,
                None => {
                    let reg = device_flow.register_client(SCOPES).await?;
                    storage.save_registration(&reg, &cx).await?;
                    reg
                }
            };

            let auth_response = device_flow
                .start_device_authorization(&registration)
                .await?;

            let prompt = DeviceFlowPrompt {
                user_code: auth_response.user_code.clone(),
                verification_uri: auth_response.verification_uri.clone(),
                expires_at: Utc::now() + chrono::Duration::seconds(auth_response.expires_in as i64),
            };

            cx.update(|cx| {
                state.update(cx, |state, cx| {
                    state.set_auth_status(AuthStatus::SigningIn { prompt });
                    cx.notify();
                });
            })?;

            let interval = Duration::from_secs(auth_response.interval);
            let expires_at =
                Utc::now() + chrono::Duration::seconds(auth_response.expires_in as i64);

            let token_result = device_flow
                .poll_for_token(
                    &registration,
                    &auth_response.device_code,
                    interval,
                    expires_at,
                )
                .await;

            match token_result {
                Ok(token) => {
                    storage.save_token(&token, &cx).await?;

                    let region_for_fetch = cx.update(|cx| {
                        state.update(cx, |state, cx| {
                            state.set_token(Some(token.clone()));
                            cx.notify();
                            state.region.clone()
                        })
                    })?;

                    KiroLanguageModelProvider::fetch_models_async(
                        http_client,
                        token,
                        region_for_fetch,
                        state,
                        &cx,
                    )
                    .await;

                    Ok(())
                }
                Err(poll_error) => {
                    let error_msg = poll_error.to_string();

                    cx.update(|cx| {
                        state.update(cx, |state, cx| {
                            state.set_auth_status(AuthStatus::Error(error_msg.clone()));
                            cx.notify();
                        });
                    })?;

                    Err(anyhow!(error_msg).into())
                }
            }
        })
    }

    fn configuration_view(
        &self,
        _target_agent: ConfigurationViewTargetAgent,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyView {
        let state = self.state.clone();
        let http_client = self.http_client.clone();
        let credentials_provider = self.credentials_provider.clone();
        cx.new(|cx| KiroConfigurationView::new(state, http_client, credentials_provider, cx))
            .into()
    }

    fn reset_credentials(&self, cx: &mut App) -> Task<Result<()>> {
        let credentials_provider = self.credentials_provider.clone();
        let state = self.state.clone();

        cx.spawn(async move |cx| {
            let storage = TokenStorage::new(credentials_provider);

            storage.delete_token(&cx).await?;
            storage.delete_registration(&cx).await?;

            cx.update(|cx| {
                state.update(cx, |state, cx| {
                    state.set_token(None);
                    state.set_registration(None);
                    state.set_auth_status(AuthStatus::SignedOut);
                    cx.notify();
                });
            })?;

            Ok(())
        })
    }
}
