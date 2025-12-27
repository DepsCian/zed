use chrono::Utc;
use credentials_provider::CredentialsProvider;
use gpui::{Context, Entity, Subscription, Task, Timer};
use http_client::HttpClient;
use kiro::{DeviceFlowClient, TokenStorage};
use std::sync::Arc;
use std::time::Duration;

use super::auth::{AuthStatus, DeviceFlowPrompt};
use super::config::SCOPES;
use super::state::KiroState;

pub struct KiroConfigurationView {
    pub(crate) state: Entity<KiroState>,
    http_client: Arc<dyn HttpClient>,
    credentials_provider: Arc<dyn CredentialsProvider>,
    pub(crate) selected_region: String,
    countdown_task: Option<Task<()>>,
    _subscription: Subscription,
}

impl KiroConfigurationView {
    pub fn new(
        state: Entity<KiroState>,
        http_client: Arc<dyn HttpClient>,
        credentials_provider: Arc<dyn CredentialsProvider>,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected_region = state.read(cx).region.clone();
        let subscription = cx.observe(&state, |_, _, cx| cx.notify());

        Self {
            state,
            http_client,
            credentials_provider,
            selected_region,
            countdown_task: None,
            _subscription: subscription,
        }
    }

    fn start_countdown(&mut self, cx: &mut Context<Self>) {
        self.countdown_task = Some(cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                let should_continue = this
                    .update(cx, |_this, cx| {
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            }
        }));
    }

    fn stop_countdown(&mut self) {
        self.countdown_task = None;
    }

    pub(crate) fn sign_in(&mut self, cx: &mut Context<Self>) {
        let http_client = self.http_client.clone();
        let credentials_provider = self.credentials_provider.clone();
        let state = self.state.clone();
        let region = self.selected_region.clone();

        self.start_countdown(cx);

        cx.spawn(async move |_this, cx| {
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

            let auth_response = device_flow.start_device_authorization(&registration).await?;

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
            let expires_at = Utc::now() + chrono::Duration::seconds(auth_response.expires_in as i64);

            let token_result = device_flow
                .poll_for_token(&registration, &auth_response.device_code, interval, expires_at)
                .await;

            match token_result {
                Ok(token) => {
                    storage.save_token(&token, &cx).await?;

                    cx.update(|cx| {
                        state.update(cx, |state, cx| {
                            state.set_token(Some(token));
                            cx.notify();
                        });
                    })?;
                }
                Err(poll_error) => {
                    let error_msg = poll_error.to_string();

                    cx.update(|cx| {
                        state.update(cx, |state, cx| {
                            state.set_auth_status(AuthStatus::Error(error_msg));
                            cx.notify();
                        });
                    })?;
                }
            }

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    pub(crate) fn sign_out(&mut self, cx: &mut Context<Self>) {
        let credentials_provider = self.credentials_provider.clone();
        let state = self.state.clone();

        self.stop_countdown();

        cx.spawn(async move |_this, cx| {
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

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    pub(crate) fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        self.stop_countdown();
        self.state.update(cx, |state, cx| {
            state.set_auth_status(AuthStatus::SignedOut);
            cx.notify();
        });
    }

    pub(crate) fn set_region(&mut self, region: String, cx: &mut Context<Self>) {
        self.selected_region = region.clone();
        self.state.update(cx, |state, cx| {
            state.region = region;
            cx.notify();
        });
    }
}
