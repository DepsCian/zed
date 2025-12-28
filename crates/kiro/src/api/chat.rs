use crate::api::client::KiroClient;
use crate::api::tools::{ToolDefinition, ToolResult, ToolUseEvent};
use crate::streaming::event_stream::AwsEventStreamParser;
use crate::types::error::{ApiError, KiroError};
use crate::types::user_context::UserContext;
use futures::{AsyncReadExt, Stream};
use serde::{Deserialize, Serialize};
use std::pin::Pin;

const SEND_MESSAGE_TARGET: &str = "AmazonCodeWhispererStreamingService.GenerateAssistantResponse";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    pub conversation_state: ConversationState,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationState {
    pub conversation_id: String,
    pub history: Vec<HistoryEntry>,
    pub current_message: CurrentMessage,
    pub chat_trigger_type: String,
    pub agent_continuation_id: String,
    pub agent_task_type: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_input_message: Option<UserInputMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_response_message: Option<AssistantResponseMessage>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentMessage {
    pub user_input_message: UserInputMessage,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputMessage {
    pub content: String,
    pub user_input_message_context: UserInputMessageContext,
    pub origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputMessageContext {
    pub env_state: EnvState,
    pub tools: Vec<ToolDefinition>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvState {
    pub operating_system: String,
    pub current_working_directory: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantResponseMessage {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_uses: Option<Vec<ToolUse>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUse {
    pub tool_use_id: String,
    pub name: String,
    pub input: serde_json::Value,
}


impl SendMessageRequest {
    pub fn new(content: String, conversation_id: Option<String>, user_context: &UserContext) -> Self {
        let os = match user_context.operating_system {
            crate::types::user_context::OperatingSystem::Linux => "linux",
            crate::types::user_context::OperatingSystem::Windows => "windows",
            crate::types::user_context::OperatingSystem::Macos => "macos",
        };

        let conv_id = conversation_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        Self {
            conversation_state: ConversationState {
                conversation_id: conv_id,
                history: Vec::new(),
                current_message: CurrentMessage {
                    user_input_message: UserInputMessage {
                        content,
                        user_input_message_context: UserInputMessageContext {
                            env_state: EnvState {
                                operating_system: os.to_string(),
                                current_working_directory: std::env::current_dir()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_else(|_| "/".to_string()),
                            },
                            tools: Vec::new(),
                            tool_results: Vec::new(),
                        },
                        origin: "KIRO_CLI".to_string(),
                        model_id: None,
                    },
                },
                chat_trigger_type: "MANUAL".to_string(),
                agent_continuation_id: uuid::Uuid::new_v4().to_string(),
                agent_task_type: "vibe".to_string(),
            },
        }
    }

    pub fn with_history(mut self, history: Vec<HistoryEntry>) -> Self {
        self.conversation_state.history = history;
        self
    }

    pub fn with_model(mut self, model_id: String) -> Self {
        self.conversation_state
            .current_message
            .user_input_message
            .model_id = Some(model_id);
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools = tools;
        self
    }

    pub fn with_tool_results(mut self, tool_results: Vec<ToolResult>) -> Self {
        self.conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tool_results = tool_results;
        self
    }
}

#[derive(Debug, Clone)]
pub enum ChatEvent {
    TextDelta { content: String },
    ToolUse { id: String, name: String, input: serde_json::Value, stop: bool },
    Metadata { conversation_id: String, message_id: String },
    Error { message: String, code: Option<String> },
    End,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssistantResponseEventPayload {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageMetadataPayload {
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default)]
    utterance_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitialResponsePayload {
    #[serde(default)]
    conversation_id: Option<String>,
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

            if let Ok(request_json) = serde_json::to_string_pretty(&request) {
                log::error!(
                    "[KIRO_API_ERROR] HTTP {} - {}\nRequest JSON:\n{}",
                    status,
                    error_text,
                    request_json
                );
            }

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
    conversation_id: Option<String>,
    pending_tool_uses: std::collections::HashMap<String, PendingToolUse>,
}

struct PendingToolUse {
    name: String,
    input_parts: Vec<String>,
}

impl<B> ChatEventStream<B> {
    fn new(body: B) -> Self {
        Self {
            body,
            parser: AwsEventStreamParser::new(),
            pending_events: Vec::new(),
            finished: false,
            conversation_id: None,
            pending_tool_uses: std::collections::HashMap::new(),
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
                this.process_aws_events(events);
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


impl<B> ChatEventStream<B> {
    fn process_aws_events(
        &mut self,
        events: Vec<Result<crate::streaming::event_stream::AwsEvent, anyhow::Error>>,
    ) {
        for event_result in events {
            match event_result {
                Ok(aws_event) => self.handle_aws_event(aws_event),
                Err(e) => self.pending_events.push(Err(KiroError::Other(e))),
            }
        }
    }

    fn handle_aws_event(&mut self, aws_event: crate::streaming::event_stream::AwsEvent) {
        if aws_event.is_exception() {
            self.handle_exception(aws_event);
            return;
        }

        if !aws_event.is_event() {
            return;
        }

        let Some(event_type) = &aws_event.event_type else {
            return;
        };

        match event_type.as_str() {
            "initial-response" => self.handle_initial_response(&aws_event),
            "assistantResponseEvent" => self.handle_assistant_response(&aws_event),
            "toolUseEvent" => self.handle_tool_use(&aws_event),
            "messageMetadataEvent" => self.handle_message_metadata(&aws_event),
            "meteringEvent" => {}
            _ => {}
        }
    }

    fn handle_exception(&mut self, aws_event: crate::streaming::event_stream::AwsEvent) {
        let exception_type = aws_event.exception_type.clone();
        let payload: ExceptionPayload = aws_event
            .parse_json()
            .unwrap_or(ExceptionPayload { message: None });

        let error_message = payload
            .message
            .unwrap_or_else(|| "Unknown error".to_string());

        let kiro_error = match exception_type.as_deref() {
            Some("ThrottlingException") => KiroError::Api(ApiError::Throttling {
                message: error_message,
                retry_after: None,
            }),
            Some("ValidationException") => {
                KiroError::Api(ApiError::Validation { message: error_message })
            }
            Some("AccessDeniedException") => {
                KiroError::Api(ApiError::AccessDenied { message: error_message })
            }
            Some("InternalServerException") => KiroError::Api(ApiError::InternalServerError),
            Some("ServiceUnavailableException") => KiroError::Api(ApiError::ServiceUnavailable),
            _ => KiroError::Network(error_message),
        };

        self.pending_events.push(Err(kiro_error));
    }

    fn handle_initial_response(&mut self, aws_event: &crate::streaming::event_stream::AwsEvent) {
        if let Ok(payload) = aws_event.parse_json::<InitialResponsePayload>() {
            if let Some(conv_id) = payload.conversation_id {
                if !conv_id.is_empty() {
                    self.conversation_id = Some(conv_id);
                }
            }
        }
    }

    fn handle_assistant_response(&mut self, aws_event: &crate::streaming::event_stream::AwsEvent) {
        if let Ok(payload) = aws_event.parse_json::<AssistantResponseEventPayload>() {
            if let Some(content) = payload.content {
                self.pending_events
                    .push(Ok(ChatEvent::TextDelta { content }));
            }
        }
    }

    fn handle_tool_use(&mut self, aws_event: &crate::streaming::event_stream::AwsEvent) {
        if let Ok(payload) = aws_event.parse_json::<ToolUseEvent>() {
            let tool_id = payload.tool_use_id.clone();
            
            let pending = self.pending_tool_uses
                .entry(tool_id.clone())
                .or_insert_with(|| PendingToolUse {
                    name: payload.name.clone(),
                    input_parts: Vec::new(),
                });
            
            if let Some(input_str) = payload.input.as_str() {
                pending.input_parts.push(input_str.to_string());
            }
            
            if payload.stop {
                let full_input = pending.input_parts.join("");
                let parsed_input: serde_json::Value = serde_json::from_str(&full_input)
                    .unwrap_or_else(|_| serde_json::json!({}));
                
                let name = pending.name.clone();
                self.pending_tool_uses.remove(&tool_id);
                
                self.pending_events.push(Ok(ChatEvent::ToolUse {
                    id: tool_id,
                    name,
                    input: parsed_input,
                    stop: true,
                }));
            }
        }
    }

    fn handle_message_metadata(&mut self, aws_event: &crate::streaming::event_stream::AwsEvent) {
        if let Ok(payload) = aws_event.parse_json::<MessageMetadataPayload>() {
            let conv_id = payload
                .conversation_id
                .or_else(|| self.conversation_id.clone())
                .unwrap_or_default();
            self.pending_events.push(Ok(ChatEvent::Metadata {
                conversation_id: conv_id,
                message_id: payload.utterance_id.unwrap_or_default(),
            }));
        }
    }
}
