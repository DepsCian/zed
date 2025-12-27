use anyhow::Result;
use serde::de::DeserializeOwned;

pub struct AwsEventStreamParser {
    _buffer: Vec<u8>,
}

impl AwsEventStreamParser {
    pub fn new() -> Self {
        Self {
            _buffer: Vec::new(),
        }
    }

    pub fn feed(&mut self, _chunk: &[u8]) -> Vec<Result<AwsEvent>> {
        todo!("Implement in task 7.1")
    }
}

impl Default for AwsEventStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct AwsEvent {
    pub message_type: String,
    pub content_type: Option<String>,
    pub event_type: Option<String>,
    pub payload: Vec<u8>,
}

impl AwsEvent {
    pub fn parse_json<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.payload)?)
    }
}
