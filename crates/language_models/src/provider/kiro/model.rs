use anyhow::{Result, anyhow};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt, TryFutureExt};
use gpui::{App, AppContext, AsyncApp, Entity};
use http_client::HttpClient;
use kiro::KiroClient;
use language_model::{
    LanguageModel, LanguageModelCompletionError, LanguageModelCompletionEvent,
    LanguageModelId, LanguageModelName, LanguageModelProviderId, LanguageModelProviderName,
    LanguageModelRequest, LanguageModelToolChoice, RateLimiter,
};
use std::sync::Arc;

use super::config::{PROVIDER_ID, PROVIDER_NAME};
use super::state::KiroState;
use super::stream::{build_send_message_request, map_kiro_error_to_completion_error, map_chat_events_to_completion_events};

pub struct KiroModel {
    pub(crate) model_id: String,
    pub(crate) model_name: String,
    pub(crate) state: Entity<KiroState>,
    pub(crate) http_client: Arc<dyn HttpClient>,
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
