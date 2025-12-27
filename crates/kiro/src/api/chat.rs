use crate::api::client::KiroClient;
use crate::streaming::event_stream::AwsEventStreamParser;
use crate::types::error::{ApiError, KiroError};
use crate::types::user_context::UserContext;
use anyhow::Result;
use futures::{AsyncReadExt, Stream};
use serde::{Deserialize, Serialize};
use std::pin::Pin;

const SEND_MESSAGE_TARGET: &str = "QDeveloperSendMessage";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub message_id: String,
    pub content: String,
    pub user_context: UserContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_arn: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ChatEvent {
    TextDelta { content: String },
    Metadata { conversation_id: String, message_id: String },
    Error { message: String, code: Option<String> },
    End,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssistantResponseEvent {
    #[serde(default)]
    assistant_response_event: Option<TextDeltaPayload>,
    #[serde(default)]
    message_metadata_event: Option<MessageMetadataPayload>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextDeltaPayload {
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageMetadataPayload {
    conversation_id: String,
    #[serde(default)]
    utterance_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExceptionPayload {
    message: Option<String>,
}

impl KiroClient {
    pub async fn send_message(
        &self,
        request: SendMessageRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatEvent, KiroError>> + Send>>, KiroError> {
        let http_request = self.build_request(SEND_MESSAGE_TARGET, &request)?;
        let response = self.http_client().send(http_request).await?;

        if !response.status().is_success() {
            let status = response.status();
            let mut body = Vec::new();
            response.into_body().read_to_end(&mut body).await?;
            let error_text = String::from_utf8_lossy(&body).to_string();

            return Err(match status.as_u16() {
                429 => KiroError::Api(ApiError::Throttling {
                    message: error_text,
                    retry_after: None,
                }),
                400 => KiroError::Api(ApiError::Validation { message: error_text }),
                401 | 403 => KiroError::Api(ApiError::AccessDenied { message: error_text }),
                500 => KiroError::Api(ApiError::InternalServerError),
                503 => KiroError::Api(ApiError::ServiceUnavailable),
                _ => KiroError::Network(format!("HTTP {}: {}", status, error_text)),
            });
        }

        let stream = ChatEventStream::new(response.into_body());
        Ok(Box::pin(stream))
    }
}

struct ChatEventStream<B> {
    body: B,
    parser: AwsEventStreamParser,
    pending_events: Vec<Result<ChatEvent, KiroError>>,
    finished: bool,
}

impl<B> ChatEventStream<B> {
    fn new(body: B) -> Self {
        Self {
            body,
            parser: AwsEventStreamParser::new(),
            pending_events: Vec::new(),
            finished: false,
        }
    }
}

impl<B: futures::AsyncRead + Unpin + Send> Stream for ChatEventStream<B> {
    type Item = Result<ChatEvent, KiroError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;

        if let Some(event) = self.pending_events.pop() {
            return Poll::Ready(Some(event));
        }

        if self.finished {
            return Poll::Ready(None);
        }

        let mut buf = [0u8; 8192];
        let this = self.get_mut();

        match Pin::new(&mut this.body).poll_read(cx, &mut buf) {
            Poll::Ready(Ok(0)) => {
                this.finished = true;
                Poll::Ready(Some(Ok(ChatEvent::End)))
            }
            Poll::Ready(Ok(n)) => {
                let events = this.parser.feed(&buf[..n]);

                for event_result in events {
                    match event_result {
                        Ok(aws_event) => {
                            if aws_event.is_exception() {
                                let exception_type = aws_event.exception_type.clone();
                                let payload: ExceptionPayload = aws_event
                                    .parse_json()
                                    .unwrap_or(ExceptionPayload { message: None });

                                let error_message = payload
                                    .message
                                    .unwrap_or_else(|| "Unknown error".to_string());

                                let kiro_error = match exception_type.as_deref() {
                                    Some("ThrottlingException") => {
                                        KiroError::Api(ApiError::Throttling {
                                            message: error_message,
                                            retry_after: None,
                                        })
                                    }
                                    Some("ValidationException") => {
                                        KiroError::Api(ApiError::Validation {
                                            message: error_message,
                                        })
                                    }
                                    Some("AccessDeniedException") => {
                                        KiroError::Api(ApiError::AccessDenied {
                                            message: error_message,
                                        })
                                    }
                                    Some("InternalServerException") => {
                                        KiroError::Api(ApiError::InternalServerError)
                                    }
                                    Some("ServiceUnavailableException") => {
                                        KiroError::Api(ApiError::ServiceUnavailable)
                                    }
                                    _ => KiroError::Network(error_message),
                                };

                                this.pending_events.push(Err(kiro_error));
                            } else if aws_event.is_event() {
                                if let Some(event_type) = &aws_event.event_type {
                                    match event_type.as_str() {
                                        "assistantResponseEvent" => {
                                            if let Ok(payload) =
                                                aws_event.parse_json::<AssistantResponseEvent>()
                                            {
                                                if let Some(text_delta) =
                                                    payload.assistant_response_event
                                                {
                                                    this.pending_events.push(Ok(
                                                        ChatEvent::TextDelta {
                                                            content: text_delta.content,
                                                        },
                                                    ));
                                                }
                                            }
                                        }
                                        "messageMetadataEvent" => {
                                            if let Ok(payload) =
                                                aws_event.parse_json::<AssistantResponseEvent>()
                                            {
                                                if let Some(metadata) =
                                                    payload.message_metadata_event
                                                {
                                                    this.pending_events.push(Ok(
                                                        ChatEvent::Metadata {
                                                            conversation_id: metadata
                                                                .conversation_id,
                                                            message_id: metadata
                                                                .utterance_id
                                                                .unwrap_or_default(),
                                                        },
                                                    ));
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            this.pending_events.push(Err(KiroError::Other(e)));
                        }
                    }
                }

                this.pending_events.reverse();

                if let Some(event) = this.pending_events.pop() {
                    Poll::Ready(Some(event))
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
            Poll::Ready(Err(e)) => {
                this.finished = true;
                Poll::Ready(Some(Err(KiroError::Network(e.to_string()))))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
