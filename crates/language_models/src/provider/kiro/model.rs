use anyhow::Result;
use credentials_provider::CredentialsProvider;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt};
use gpui::{App, AppContext, AsyncApp, Entity};
use http_client::HttpClient;
use kiro::KiroClient;
use language_model::{
    LanguageModel, LanguageModelCacheConfiguration, LanguageModelCompletionError,
    LanguageModelCompletionEvent, LanguageModelId, LanguageModelName, LanguageModelProviderId,
    LanguageModelProviderName, LanguageModelRequest, LanguageModelToolChoice, RateLimiter,
};
use std::sync::Arc;

use super::config::{PROVIDER_ID, PROVIDER_NAME};
use super::provider::KiroLanguageModelProvider;
use super::state::KiroState;
use super::stream::{
    build_send_message_request, map_chat_events_to_completion_events,
    map_kiro_error_to_completion_error,
};

pub struct KiroModel {
    pub(crate) model_id: String,
    pub(crate) model_name: String,
    pub(crate) max_tokens: u64,
    pub(crate) state: Entity<KiroState>,
    pub(crate) http_client: Arc<dyn HttpClient>,
    pub(crate) credentials_provider: Arc<dyn CredentialsProvider>,
    pub(crate) request_limiter: RateLimiter,
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
        true
    }

    fn supports_images(&self) -> bool {
        true
    }

    fn supports_streaming_tools(&self) -> bool {
        true
    }

    fn supports_tool_choice(&self, choice: LanguageModelToolChoice) -> bool {
        match choice {
            LanguageModelToolChoice::Auto | LanguageModelToolChoice::None => true,
            LanguageModelToolChoice::Any => false,
        }
    }

    fn telemetry_id(&self) -> String {
        format!("kiro/{}", self.model_id)
    }

    fn max_token_count(&self) -> u64 {
        self.max_tokens
    }

    fn max_output_tokens(&self) -> Option<u64> {
        Some(8192)
    }

    fn cache_configuration(&self) -> Option<LanguageModelCacheConfiguration> {
        None
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
        let credentials_provider = self.credentials_provider.clone();
        let request_limiter = self.request_limiter.clone();
        let model_id = self.model_id.clone();

        cx.spawn(async move |cx| {
            let token_result = KiroLanguageModelProvider::ensure_valid_token(
                http_client.clone(),
                credentials_provider,
                state,
                &cx,
            ).await;

            let (token, region) = match token_result {
                Some(data) => data,
                None => {
                    return Err(LanguageModelCompletionError::AuthenticationError {
                        provider: PROVIDER_NAME,
                        message: "Token expired and refresh failed".to_string(),
                    });
                }
            };

            let kiro_request = build_send_message_request(&request, &model_id);

            let stream_result = request_limiter.stream(async move {
                let client = KiroClient::new(http_client, token, region);
                let stream = client.send_message(kiro_request).await.map_err(map_kiro_error_to_completion_error)?;
                Ok(map_chat_events_to_completion_events(stream))
            }).await;

            match stream_result {
                Ok(stream) => Ok(stream.boxed()),
                Err(e) => Err(e),
            }
        }).boxed()
    }
}
