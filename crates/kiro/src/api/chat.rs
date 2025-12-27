use crate::types::user_context::UserContext;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatEvent {
    #[serde(rename = "textDelta")]
    TextDelta { content: String },
    #[serde(rename = "metadata")]
    Metadata {
        conversation_id: String,
        message_id: String,
    },
    #[serde(rename = "error")]
    Error {
        message: String,
        code: Option<String>,
    },
    #[serde(rename = "end")]
    End,
}
