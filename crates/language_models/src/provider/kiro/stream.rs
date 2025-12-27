use anyhow::anyhow;
use futures::Stream;
use kiro::{
    ApiError, ChatEvent, KiroError, SendMessageRequest, ToolDefinition, ToolResult,
    ToolResultContent, UserContext,
};
use language_model::{
    LanguageModelCompletionError, LanguageModelCompletionEvent, LanguageModelRequest,
    LanguageModelToolResultContent, LanguageModelToolUse, MessageContent, Role, StopReason,
};
use std::pin::Pin;

use super::config::PROVIDER_NAME;

fn normalize_tool_input(tool_use: &LanguageModelToolUse) -> serde_json::Value {
    if let serde_json::Value::Object(obj) = &tool_use.input {
        if !obj.is_empty() {
            return tool_use.input.clone();
        }
    }
    
    if !tool_use.raw_input.is_empty() {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&tool_use.raw_input) {
            if parsed.is_object() {
                return parsed;
            }
        }
    }
    
    serde_json::json!({})
}

pub fn build_send_message_request(
    request: &LanguageModelRequest,
    model_id: &str,
) -> SendMessageRequest {
    let user_context = UserContext::for_zed();
    let (history, current_content, tool_results) = build_conversation_parts(request);

    let mut send_request =
        SendMessageRequest::new(current_content, request.thread_id.clone(), &user_context);

    if model_id != "auto" {
        send_request = send_request.with_model(model_id.to_string());
    }

    if !history.is_empty() {
        send_request = send_request.with_history(history);
    }

    if !request.tools.is_empty() {
        let tools: Vec<ToolDefinition> = request
            .tools
            .iter()
            .map(|t| ToolDefinition::new(t.name.clone(), t.description.clone(), t.input_schema.clone()))
            .collect();
        send_request = send_request.with_tools(tools);
    }

    if !tool_results.is_empty() {
        send_request = send_request.with_tool_results(tool_results);
    }

    send_request
}

fn build_conversation_parts(request: &LanguageModelRequest) -> (Vec<kiro::HistoryEntry>, String, Vec<ToolResult>) {
    use kiro::{HistoryEntry, UserInputMessage, AssistantResponseMessage, UserInputMessageContext, EnvState, ToolUse};
    
    let mut history = Vec::new();
    let mut current_content = String::new();
    let mut tool_results = Vec::new();
    let mut pending_assistant_content = String::new();
    let mut pending_assistant_tool_uses: Vec<ToolUse> = Vec::new();
    let mut system_prompt = String::new();
    
    for message in &request.messages {
        if message.role == Role::System {
            for content in &message.content {
                if let MessageContent::Text(text) = content {
                    if !text.is_empty() {
                        if !system_prompt.is_empty() {
                            system_prompt.push_str("\n\n");
                        }
                        system_prompt.push_str(text);
                    }
                }
            }
        }
    }
    
    let non_system_messages: Vec<_> = request.messages.iter()
        .filter(|m| m.role != Role::System)
        .collect();
    
    for (i, message) in non_system_messages.iter().enumerate() {
        let is_last = i == non_system_messages.len() - 1;
        let is_first_user = i == 0 && message.role == Role::User;
        
        match message.role {
            Role::User => {
                if !pending_assistant_content.is_empty() || !pending_assistant_tool_uses.is_empty() {
                    history.push(HistoryEntry {
                        user_input_message: None,
                        assistant_response_message: Some(AssistantResponseMessage {
                            content: pending_assistant_content.clone(),
                            message_id: None,
                            tool_uses: if pending_assistant_tool_uses.is_empty() { 
                                None 
                            } else { 
                                Some(pending_assistant_tool_uses.clone()) 
                            },
                        }),
                    });
                    pending_assistant_content.clear();
                    pending_assistant_tool_uses.clear();
                }
                
                let mut user_text = String::new();
                let mut msg_tool_results = Vec::new();
                
                for content in &message.content {
                    match content {
                        MessageContent::Text(text) => {
                            if !text.is_empty() {
                                user_text.push_str(text);
                            }
                        }
                        MessageContent::ToolResult(result) => {
                            let content_text = match &result.content {
                                LanguageModelToolResultContent::Text(text) => text.to_string(),
                                LanguageModelToolResultContent::Image(_) => "[image]".to_string(),
                            };
                            let status = if result.is_error { "error" } else { "success" };
                            msg_tool_results.push(ToolResult {
                                tool_use_id: result.tool_use_id.to_string(),
                                content: vec![ToolResultContent {
                                    text: content_text,
                                    status: Some(status.to_string()),
                                }],
                                status: status.to_string(),
                            });
                        }
                        _ => {}
                    }
                }
                
                if is_last {
                    let final_content = if is_first_user && !system_prompt.is_empty() {
                        format!("{}\n\n{}", system_prompt, user_text)
                    } else {
                        user_text
                    };
                    current_content = if msg_tool_results.is_empty() { final_content } else { String::new() };
                    tool_results = msg_tool_results;
                } else if !user_text.is_empty() || !msg_tool_results.is_empty() {
                    let final_content = if is_first_user && !system_prompt.is_empty() {
                        format!("{}\n\n{}", system_prompt, user_text)
                    } else {
                        user_text
                    };
                    history.push(HistoryEntry {
                        user_input_message: Some(UserInputMessage {
                            content: final_content,
                            user_input_message_context: UserInputMessageContext {
                                env_state: EnvState {
                                    operating_system: "linux".to_string(),
                                    current_working_directory: std::env::current_dir()
                                        .map(|p| p.to_string_lossy().to_string())
                                        .unwrap_or_else(|_| "/".to_string()),
                                },
                                tools: Vec::new(),
                                tool_results: msg_tool_results,
                            },
                            origin: "KIRO_CLI".to_string(),
                            model_id: None,
                        }),
                        assistant_response_message: None,
                    });
                }
            }
            Role::Assistant => {
                for content in &message.content {
                    match content {
                        MessageContent::Text(text) => {
                            if !text.is_empty() {
                                pending_assistant_content.push_str(text);
                            }
                        }
                        MessageContent::ToolUse(tool_use) => {
                            let input = normalize_tool_input(tool_use);
                            pending_assistant_tool_uses.push(ToolUse {
                                tool_use_id: tool_use.id.to_string(),
                                name: tool_use.name.to_string(),
                                input,
                            });
                        }
                        _ => {}
                    }
                }
            }
            Role::System => {}
        }
    }
    
    if !pending_assistant_content.is_empty() || !pending_assistant_tool_uses.is_empty() {
        history.push(HistoryEntry {
            user_input_message: None,
            assistant_response_message: Some(AssistantResponseMessage {
                content: pending_assistant_content,
                message_id: None,
                tool_uses: if pending_assistant_tool_uses.is_empty() { 
                    None 
                } else { 
                    Some(pending_assistant_tool_uses) 
                },
            }),
        });
    }
    
    (history, current_content, tool_results)
}


pub fn map_api_error_to_completion_error(error: ApiError) -> LanguageModelCompletionError {
    use std::time::Duration;

    match error {
        ApiError::Throttling {
            message: _,
            retry_after,
        } => LanguageModelCompletionError::RateLimitExceeded {
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

    let mut stop_reason = StopReason::EndTurn;

    stream.map(move |result| match result {
        Ok(ChatEvent::TextDelta { content }) => Ok(LanguageModelCompletionEvent::Text(content)),
        Ok(ChatEvent::ToolUse { id, name, input, stop }) => {
            if stop {
                stop_reason = StopReason::ToolUse;
            }
            Ok(LanguageModelCompletionEvent::ToolUse(LanguageModelToolUse {
                id: id.into(),
                name: name.into(),
                is_input_complete: true,
                input,
                raw_input: String::new(),
                thought_signature: None,
            }))
        }
        Ok(ChatEvent::Metadata { .. }) => Ok(LanguageModelCompletionEvent::Started),
        Ok(ChatEvent::Error { message, code }) => Err(LanguageModelCompletionError::Other(anyhow!(
            "Kiro API error: {} (code: {:?})",
            message,
            code
        ))),
        Ok(ChatEvent::End) => Ok(LanguageModelCompletionEvent::Stop(stop_reason)),
        Err(e) => Err(map_kiro_error_to_completion_error(e)),
    })
}
