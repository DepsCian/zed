use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use credentials_provider::CredentialsProvider;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt, Stream, StreamExt, TryFutureExt};
use gpui::{AnyView, App, AsyncApp, ClipboardItem, Context, Entity, Subscription, Task, Timer};
use http_client::HttpClient;
use kiro::{
    ApiError, BuilderIdToken, ChatEvent, DeviceFlowClient, DeviceRegistration, KiroClient,
    KiroError, ModelInfo, SendMessageRequest, TokenStorage, UserContext,
};
use language_model::{
    AuthenticateError, ConfigurationViewTargetAgent, IconOrSvg, LanguageModel,
    LanguageModelCompletionError, LanguageModelCompletionEvent, LanguageModelId, LanguageModelName,
    LanguageModelProvider, LanguageModelProviderId, LanguageModelProviderName,
    LanguageModelProviderState, LanguageModelRequest, LanguageModelToolChoice, RateLimiter,
    StopReason,
};
use settings::SettingsStore;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use ui::{ButtonLike, ConfiguredApiCard, prelude::*};

const PROVIDER_ID: LanguageModelProviderId = LanguageModelProviderId::new("kiro");
const PROVIDER_NAME: LanguageModelProviderName = LanguageModelProviderName::new("Kiro AI");

const DEFAULT_REGION: &str = "us-east-1";
const SCOPES: &[&str] = &["codewhisperer:completions", "codewhisperer:analysis", "codewhisperer:conversations"];

const AWS_BUILDER_ID_URL: &str = "https://view.awsapps.com/start";
const AVAILABLE_REGIONS: &[(&str, &str)] = &[
    ("us-east-1", "US East (N. Virginia)"),
    ("eu-central-1", "EU (Frankfurt)"),
];

pub use settings::KiroAvailableModel as AvailableModel;

#[derive(Default, Debug, Clone, PartialEq)]
pub struct KiroSettings {
    pub region: String,
    pub available_models: Vec<AvailableModel>,
}

#[derive(Debug, Clone)]
pub struct DeviceFlowPrompt {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum AuthStatus {
    SignedOut,
    SigningIn { prompt: DeviceFlowPrompt },
    Authenticated,
    Error(String),
}

impl Default for AuthStatus {
    fn default() -> Self {
        Self::SignedOut
    }
}

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
    token: Option<BuilderIdToken>,
    registration: Option<DeviceRegistration>,
    auth_status: AuthStatus,
    region: String,
    available_models: Vec<KiroModelDefinition>,
    default_model_id: Option<String>,
    models_loaded: bool,
    _settings_subscription: Subscription,
}

impl KiroState {
    fn is_authenticated(&self) -> bool {
        match &self.token {
            Some(token) => !token.is_expired(),
            None => false,
        }
    }

    fn set_token(&mut self, token: Option<BuilderIdToken>) {
        self.token = token;
        self.auth_status = if self.token.is_some() {
            AuthStatus::Authenticated
        } else {
            AuthStatus::SignedOut
        };
    }

    fn set_registration(&mut self, registration: Option<DeviceRegistration>) {
        self.registration = registration;
    }

    fn set_auth_status(&mut self, status: AuthStatus) {
        self.auth_status = status;
    }

    fn set_available_models(&mut self, models: Vec<KiroModelDefinition>, default_id: Option<String>) {
        self.available_models = models;
        self.default_model_id = default_id;
        self.models_loaded = true;
    }
}

pub struct KiroLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    credentials_provider: Arc<dyn CredentialsProvider>,
    state: Entity<KiroState>,
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

    async fn fetch_models_async(
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

    fn create_language_model(&self, model_def: &KiroModelDefinition) -> Arc<dyn LanguageModel> {
        Arc::new(KiroModel {
            model_id: model_def.id.clone(),
            model_name: model_def.name.clone(),
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }

    fn create_default_model(&self) -> Arc<dyn LanguageModel> {
        Arc::new(KiroModel {
            model_id: "auto".to_string(),
            model_name: "Auto".to_string(),
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }

    fn region(&self, cx: &App) -> String {
        self.state.read(cx).region.clone()
    }

    fn refresh_models(&self, cx: &mut App) {
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

impl LanguageModelProvider for KiroLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        PROVIDER_ID
    }

    fn name(&self) -> LanguageModelProviderName {
        PROVIDER_NAME
    }

    fn icon(&self) -> IconOrSvg {
        IconOrSvg::Icon(IconName::AiZed)
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
        
        let fast_model = state.available_models.iter()
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
        
        state.available_models
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
                    ).await;

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
        cx.new(|cx| KiroConfigurationView::new(state, http_client, credentials_provider, cx)).into()
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

struct KiroConfigurationView {
    state: Entity<KiroState>,
    http_client: Arc<dyn HttpClient>,
    credentials_provider: Arc<dyn CredentialsProvider>,
    selected_region: String,
    countdown_task: Option<Task<()>>,
    _subscription: Subscription,
}

impl KiroConfigurationView {
    fn new(
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

    fn sign_in(&mut self, cx: &mut Context<Self>) {
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

    fn sign_out(&mut self, cx: &mut Context<Self>) {
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

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        self.stop_countdown();
        self.state.update(cx, |state, cx| {
            state.set_auth_status(AuthStatus::SignedOut);
            cx.notify();
        });
    }

    fn set_region(&mut self, region: String, cx: &mut Context<Self>) {
        self.selected_region = region.clone();
        self.state.update(cx, |state, cx| {
            state.region = region;
            cx.notify();
        });
    }

    fn render_instructions(&self) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(Label::new(
                "Kiro AI provides AI-powered coding assistance through AWS Builder ID authentication.",
            ))
            .child(Label::new(
                "Sign in with your AWS Builder ID to access Kiro's AI features.",
            ))
    }

    fn render_region_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current_region = self.selected_region.clone();

        v_flex()
            .gap_1()
            .child(Label::new("Region").size(LabelSize::Small).color(Color::Muted))
            .child(
                h_flex()
                    .gap_2()
                    .children(AVAILABLE_REGIONS.iter().map(|(region, label)| {
                        let is_selected = current_region == *region;
                        let region_str = region.to_string();

                        Button::new(SharedString::from(*region), *label)
                            .style(if is_selected {
                                ButtonStyle::Filled
                            } else {
                                ButtonStyle::Outlined
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_region(region_str.clone(), cx);
                            }))
                    })),
            )
    }

    fn render_device_code_ui(&self, prompt: &DeviceFlowPrompt, cx: &mut Context<Self>) -> impl IntoElement {
        let user_code = prompt.user_code.clone();
        let verification_uri = prompt.verification_uri.clone();
        let expires_at = prompt.expires_at;

        let remaining_seconds = (expires_at - Utc::now()).num_seconds().max(0);
        let minutes = remaining_seconds / 60;
        let seconds = remaining_seconds % 60;

        let copied = cx
            .read_from_clipboard()
            .map(|item| item.text().as_ref() == Some(&user_code))
            .unwrap_or(false);

        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Enter this code on AWS:").color(Color::Muted))
                    .child(
                        ButtonLike::new("copy-code")
                            .style(ButtonStyle::Tinted(ui::TintColor::Accent))
                            .child(
                                h_flex()
                                    .w_full()
                                    .px_3()
                                    .py_2()
                                    .justify_between()
                                    .child(
                                        Label::new(user_code.clone())
                                            .size(LabelSize::Large)
                                            .weight(gpui::FontWeight::BOLD),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(Icon::new(IconName::Copy).size(IconSize::Small))
                                            .child(Label::new(if copied { "Copied!" } else { "Copy" })),
                                    ),
                            )
                            .on_click({
                                let code = user_code.clone();
                                move |_, _window: &mut Window, cx: &mut App| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                                }
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("open-aws", "Open AWS")
                            .style(ButtonStyle::Outlined)
                            .icon(IconName::ArrowUpRight)
                            .icon_size(IconSize::Small)
                            .icon_position(IconPosition::End)
                            .on_click({
                                let uri = verification_uri.clone();
                                move |_, _, cx| cx.open_url(&uri)
                            }),
                    )
                    .child(
                        Button::new("cancel-sign-in", "Cancel")
                            .style(ButtonStyle::Subtle)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_sign_in(cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(IconName::CountdownTimer).size(IconSize::Small).color(Color::Muted))
                    .child(
                        Label::new(format!("Expires in {}:{:02}", minutes, seconds))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
    }

    fn render_sign_in_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("sign-in", "Sign in with AWS Builder ID")
            .full_width()
            .style(ButtonStyle::Outlined)
            .icon(IconName::Person)
            .icon_position(IconPosition::Start)
            .icon_size(IconSize::Small)
            .on_click(cx.listener(|this, _, _, cx| {
                this.sign_in(cx);
            }))
    }

    fn render_error(&self, message: &str, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(IconName::Warning).color(Color::Error))
                    .child(Label::new(format!("Error: {}", message)).color(Color::Error)),
            )
            .child(self.render_sign_in_button(cx))
    }
}

impl Render for KiroConfigurationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let auth_status = self.state.read(cx).auth_status.clone();
        let is_authenticated = self.state.read(cx).is_authenticated();

        v_flex()
            .gap_3()
            .child(self.render_instructions())
            .child(self.render_region_selector(cx))
            .child(match auth_status {
                AuthStatus::SignedOut => {
                    self.render_sign_in_button(cx).into_any_element()
                }
                AuthStatus::SigningIn { prompt } => {
                    self.render_device_code_ui(&prompt, cx).into_any_element()
                }
                AuthStatus::Authenticated => {
                    if is_authenticated {
                        ConfiguredApiCard::new("Authenticated with AWS Builder ID")
                            .button_label("Sign Out")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sign_out(cx);
                            }))
                            .into_any_element()
                    } else {
                        self.render_sign_in_button(cx).into_any_element()
                    }
                }
                AuthStatus::Error(msg) => {
                    self.render_error(&msg, cx).into_any_element()
                }
            })
            .child(
                Button::new("aws-builder-id-info", "Learn about AWS Builder ID")
                    .style(ButtonStyle::Subtle)
                    .icon(IconName::ArrowUpRight)
                    .icon_size(IconSize::XSmall)
                    .icon_color(Color::Muted)
                    .on_click(move |_, _, cx| cx.open_url(AWS_BUILDER_ID_URL)),
            )
    }
}

pub struct KiroModel {
    model_id: String,
    model_name: String,
    state: Entity<KiroState>,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
}

impl LanguageModel for KiroModel {
    fn id(&self) -> LanguageModelId {
        LanguageModelId::from(format!("kiro-{}", self.model_id))
    }

    fn name(&self) -> LanguageModelName {
        LanguageModelName::from(self.model_name.clone())
    }

    fn provider_id(&self) -> LanguageModelProviderId {
        PROVIDER_ID
    }

    fn provider_name(&self) -> LanguageModelProviderName {
        PROVIDER_NAME
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn supports_images(&self) -> bool {
        false
    }

    fn supports_tool_choice(&self, _choice: LanguageModelToolChoice) -> bool {
        false
    }

    fn telemetry_id(&self) -> String {
        format!("kiro/{}", self.model_id)
    }

    fn max_token_count(&self) -> u64 {
        128000
    }

    fn count_tokens(
        &self,
        request: LanguageModelRequest,
        cx: &App,
    ) -> BoxFuture<'static, Result<u64>> {
        let messages = request
            .messages
            .into_iter()
            .map(|message| tiktoken_rs::ChatCompletionRequestMessage {
                role: match message.role {
                    language_model::Role::User => "user".into(),
                    language_model::Role::Assistant => "assistant".into(),
                    language_model::Role::System => "system".into(),
                },
                content: Some(message.string_contents()),
                name: None,
                function_call: None,
            })
            .collect::<Vec<_>>();

        cx.background_spawn(async move {
            tiktoken_rs::num_tokens_from_messages("gpt-4", &messages).map(|tokens| tokens as u64)
        })
        .boxed()
    }

    fn stream_completion(
        &self,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<
            BoxStream<'static, Result<LanguageModelCompletionEvent, LanguageModelCompletionError>>,
            LanguageModelCompletionError,
        >,
    > {
        let state = self.state.clone();
        let http_client = self.http_client.clone();
        let request_limiter = self.request_limiter.clone();
        let model_id = self.model_id.clone();

        let state_result = state.read_with(cx, |state, _cx| {
            match &state.token {
                Some(token) if !token.is_expired() => Ok((token.clone(), state.region.clone())),
                Some(_) => Err(LanguageModelCompletionError::AuthenticationError {
                    provider: PROVIDER_NAME,
                    message: "Token expired".to_string(),
                }),
                None => Err(LanguageModelCompletionError::NoApiKey {
                    provider: PROVIDER_NAME,
                }),
            }
        });

        let (token, region) = match state_result {
            Ok(Ok(data)) => data,
            Ok(Err(e)) => return futures::future::ready(Err(e)).boxed(),
            Err(_) => {
                return futures::future::ready(Err(LanguageModelCompletionError::Other(anyhow!(
                    "App state dropped"
                ))))
                .boxed()
            }
        };

        let kiro_request = build_send_message_request(&request, &model_id);

        let future = request_limiter.stream(async move {
            let client = KiroClient::new(http_client, token, region);
            let stream = client.send_message(kiro_request).await.map_err(map_kiro_error_to_completion_error)?;
            Ok(map_chat_events_to_completion_events(stream))
        });

        future.map_ok(|f| f.boxed()).boxed()
    }
}

fn build_send_message_request(request: &LanguageModelRequest, model_id: &str) -> SendMessageRequest {
    let content = request
        .messages
        .iter()
        .map(|msg| {
            let role_prefix = match msg.role {
                language_model::Role::User => "",
                language_model::Role::Assistant => "Assistant: ",
                language_model::Role::System => "System: ",
            };
            format!("{}{}", role_prefix, msg.string_contents())
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let user_context = UserContext::for_zed();
    let mut send_request = SendMessageRequest::new(content, request.thread_id.clone(), &user_context);
    
    if model_id != "auto" {
        send_request = send_request.with_model(model_id.to_string());
    }
    
    send_request
}

fn map_api_error_to_completion_error(error: ApiError) -> LanguageModelCompletionError {
    match error {
        ApiError::Throttling { message: _, retry_after } => LanguageModelCompletionError::RateLimitExceeded {
            provider: PROVIDER_NAME,
            retry_after: retry_after.map(Duration::from_secs),
        },
        ApiError::Validation { message } => LanguageModelCompletionError::BadRequestFormat {
            provider: PROVIDER_NAME,
            message,
        },
        ApiError::AccessDenied { message } => LanguageModelCompletionError::AuthenticationError {
            provider: PROVIDER_NAME,
            message,
        },
        ApiError::InternalServerError => LanguageModelCompletionError::ApiInternalServerError {
            provider: PROVIDER_NAME,
            message: "Internal server error".to_string(),
        },
        ApiError::ServiceUnavailable => LanguageModelCompletionError::ServerOverloaded {
            provider: PROVIDER_NAME,
            retry_after: None,
        },
    }
}

fn map_kiro_error_to_completion_error(error: KiroError) -> LanguageModelCompletionError {
    match error {
        KiroError::Api(api_error) => map_api_error_to_completion_error(api_error),
        KiroError::Auth(auth_error) => LanguageModelCompletionError::AuthenticationError {
            provider: PROVIDER_NAME,
            message: auth_error.to_string(),
        },
        KiroError::Network(msg) => LanguageModelCompletionError::HttpSend {
            provider: PROVIDER_NAME,
            error: anyhow!(msg),
        },
        KiroError::Io(e) => LanguageModelCompletionError::HttpSend {
            provider: PROVIDER_NAME,
            error: anyhow!(e),
        },
        KiroError::Other(e) => LanguageModelCompletionError::Other(e),
    }
}

fn map_chat_events_to_completion_events(
    stream: Pin<Box<dyn Stream<Item = Result<ChatEvent, KiroError>> + Send>>,
) -> impl Stream<Item = Result<LanguageModelCompletionEvent, LanguageModelCompletionError>> {
    stream.map(|result| match result {
        Ok(ChatEvent::TextDelta { content }) => {
            Ok(LanguageModelCompletionEvent::Text(content))
        }
        Ok(ChatEvent::Metadata { .. }) => Ok(LanguageModelCompletionEvent::Started),
        Ok(ChatEvent::End) => Ok(LanguageModelCompletionEvent::Stop(StopReason::EndTurn)),
        Ok(ChatEvent::Error { message, code }) => {
            Err(LanguageModelCompletionError::Other(anyhow!(
                "Kiro API error: {} (code: {:?})",
                message,
                code
            )))
        }
        Err(e) => Err(map_kiro_error_to_completion_error(e)),
    })
}
