use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub tool_specification: ToolSpecification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpecification {
    pub name: String,
    pub description: String,
    pub input_schema: InputSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputSchema {
    pub json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: Vec<ToolResultContent>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultContent {
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUseEvent {
    pub tool_use_id: String,
    pub name: String,
    #[serde(default)]
    pub input: serde_json::Value,
    #[serde(default)]
    pub stop: bool,
}

impl ToolDefinition {
    pub fn new(name: String, description: String, input_schema: serde_json::Value) -> Self {
        Self {
            tool_specification: ToolSpecification {
                name,
                description,
                input_schema: InputSchema { json: input_schema },
            },
        }
    }
}

impl ToolResult {
    pub fn success(tool_use_id: String, text: String) -> Self {
        Self {
            tool_use_id,
            content: vec![ToolResultContent { text }],
            status: "success".to_string(),
        }
    }

    pub fn error(tool_use_id: String, text: String) -> Self {
        Self {
            tool_use_id,
            content: vec![ToolResultContent { text }],
            status: "error".to_string(),
        }
    }
}
