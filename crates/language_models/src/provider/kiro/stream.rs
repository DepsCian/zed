use anyhow::anyhow;
use futures::Stream;
use kiro::{ApiError, ChatEvent, KiroError, SendMessageRequest, UserContext};
use language_model::{
    LanguageModelCompletionError, LanguageModelCompletionEvent, LanguageModelRequest, StopReason,
};
use std::pin::Pin;

use super::config::PROVIDER_NAME;

pub fn build_send_message_request(request: &LanguageModelRequest, model_id: &str) -> SendMessageRequest {
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

pub fn map_api_error_to_completion_error(error: ApiError) -> LanguageModelCompletionError {
    use std::time::Duration;
    
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

pub fn map_kiro_error_to_completion_error(error: KiroError) -> LanguageModelCompletionError {
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

pub fn map_chat_events_to_completion_events(
    stream: Pin<Box<dyn Stream<Item = Result<ChatEvent, KiroError>> + Send>>,
) -> impl Stream<Item = Result<LanguageModelCompletionEvent, LanguageModelCompletionError>> {
    use futures::StreamExt;
    
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
