use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use credentials_provider::CredentialsProvider;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt, Stream, StreamExt, TryFutureExt};
use gpui::{AnyView, App, AsyncApp, Context, Entity, Subscription, Task};
use http_client::HttpClient;
use kiro::{
    ApiError, BuilderIdToken, ChatEvent, DeviceFlowClient, DeviceRegistration, KiroClient,
    KiroError, SendMessageRequest, TokenStorage, UserContext,
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use ui::prelude::*;

static MESSAGE_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

const PROVIDER_ID: LanguageModelProviderId = LanguageModelProviderId::new("kiro");
const PROVIDER_NAME: LanguageModelProviderName = LanguageModelProviderName::new("Kiro AI");

const DEFAULT_REGION: &str = "us-east-1";
const SCOPES: &[&str] = &["codewhisperer:completions", "codewhisperer:conversations"];

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

pub struct KiroState {
    token: Option<BuilderIdToken>,
    registration: Option<DeviceRegistration>,
    auth_status: AuthStatus,
    region: String,
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

        cx.spawn(async move |cx| {
            let token = storage.load_token(&cx).await.ok().flatten();
            let registration = storage.load_registration(&cx).await.ok().flatten();

            cx.update(|cx| {
                state.update(cx, |state, cx| {
                    state.set_token(token);
                    state.set_registration(registration);
                    cx.notify();
                });
            })
            .ok();
        })
        .detach();
    }

    fn create_language_model(&self) -> Arc<dyn LanguageModel> {
        Arc::new(KiroModel {
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }

    fn region(&self, cx: &App) -> String {
        self.state.read(cx).region.clone()
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

    fn default_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(self.create_language_model())
    }

    fn default_fast_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(self.create_language_model())
    }

    fn provided_models(&self, _cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        vec![self.create_language_model()]
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        if self.is_authenticated(cx) {
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

                    cx.update(|cx| {
                        state.update(cx, |state, cx| {
                            state.set_token(Some(token));
                            cx.notify();
                        });
                    })?;

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
        cx.new(|_cx| KiroConfigurationView { state }).into()
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
}

impl Render for KiroConfigurationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let auth_status = self.state.read(cx).auth_status.clone();

        v_flex()
            .gap_2()
            .child(Label::new("Kiro AI").size(LabelSize::Large))
            .child(match auth_status {
                AuthStatus::SignedOut => {
                    Label::new("Not signed in. Click authenticate to sign in with AWS Builder ID.")
                }
                AuthStatus::SigningIn { prompt } => {
                    Label::new(format!(
                        "Enter code {} at {}",
                        prompt.user_code, prompt.verification_uri
                    ))
                }
                AuthStatus::Authenticated => Label::new("Authenticated with AWS Builder ID"),
                AuthStatus::Error(msg) => Label::new(format!("Error: {}", msg)),
            })
    }
}

pub struct KiroModel {
    state: Entity<KiroState>,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
}

impl LanguageModel for KiroModel {
    fn id(&self) -> LanguageModelId {
        LanguageModelId::from("kiro-default".to_string())
    }

    fn name(&self) -> LanguageModelName {
        LanguageModelName::from("Kiro".to_string())
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
        "kiro/kiro-default".to_string()
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

        let kiro_request = build_send_message_request(&request);

        let future = request_limiter.stream(async move {
            let client = KiroClient::new(http_client, token, region);
            let stream = client.send_message(kiro_request).await.map_err(map_kiro_error_to_completion_error)?;
            Ok(map_chat_events_to_completion_events(stream))
        });

        future.map_ok(|f| f.boxed()).boxed()
    }
}

fn build_send_message_request(request: &LanguageModelRequest) -> SendMessageRequest {
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

    let message_id = format!("msg-{}", MESSAGE_ID_COUNTER.fetch_add(1, Ordering::SeqCst));

    SendMessageRequest {
        conversation_id: request.thread_id.clone(),
        message_id,
        content,
        user_context: UserContext::for_zed(),
        profile_arn: None,
    }
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
