use anyhow::{anyhow, Result};
use serde::de::DeserializeOwned;
use std::io::{Cursor, Read};

const PRELUDE_LENGTH: usize = 8;
const PRELUDE_CRC_LENGTH: usize = 4;
const MESSAGE_CRC_LENGTH: usize = 4;
const MIN_MESSAGE_LENGTH: usize = PRELUDE_LENGTH + PRELUDE_CRC_LENGTH + MESSAGE_CRC_LENGTH;

const HEADER_TYPE_BOOL_TRUE: u8 = 0;
const HEADER_TYPE_BOOL_FALSE: u8 = 1;
const HEADER_TYPE_BYTE: u8 = 2;
const HEADER_TYPE_SHORT: u8 = 3;
const HEADER_TYPE_INT: u8 = 4;
const HEADER_TYPE_LONG: u8 = 5;
const HEADER_TYPE_BYTES: u8 = 6;
const HEADER_TYPE_STRING: u8 = 7;
const HEADER_TYPE_TIMESTAMP: u8 = 8;
const HEADER_TYPE_UUID: u8 = 9;

pub struct AwsEventStreamParser {
    buffer: Vec<u8>,
}

impl AwsEventStreamParser {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Result<AwsEvent>> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();

        loop {
            match self.try_parse_message() {
                Ok(Some(event)) => events.push(Ok(event)),
                Ok(None) => break,
                Err(e) => {
                    events.push(Err(e));
                    break;
                }
            }
        }

        events
    }

    fn try_parse_message(&mut self) -> Result<Option<AwsEvent>> {
        if self.buffer.len() < MIN_MESSAGE_LENGTH {
            return Ok(None);
        }

        let total_length = u32::from_be_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;

        if total_length < MIN_MESSAGE_LENGTH {
            return Err(anyhow!("Invalid message length: {}", total_length));
        }

        if self.buffer.len() < total_length {
            return Ok(None);
        }

        let headers_length = u32::from_be_bytes([
            self.buffer[4],
            self.buffer[5],
            self.buffer[6],
            self.buffer[7],
        ]) as usize;

        let prelude_crc = u32::from_be_bytes([
            self.buffer[8],
            self.buffer[9],
            self.buffer[10],
            self.buffer[11],
        ]);

        let computed_prelude_crc = crc32c(&self.buffer[0..8]);
        if prelude_crc != computed_prelude_crc {
            return Err(anyhow!(
                "Prelude CRC mismatch: expected {}, got {}",
                prelude_crc,
                computed_prelude_crc
            ));
        }

        let message_crc_offset = total_length - MESSAGE_CRC_LENGTH;
        let message_crc = u32::from_be_bytes([
            self.buffer[message_crc_offset],
            self.buffer[message_crc_offset + 1],
            self.buffer[message_crc_offset + 2],
            self.buffer[message_crc_offset + 3],
        ]);

        let computed_message_crc = crc32c(&self.buffer[0..message_crc_offset]);
        if message_crc != computed_message_crc {
            return Err(anyhow!(
                "Message CRC mismatch: expected {}, got {}",
                message_crc,
                computed_message_crc
            ));
        }

        let headers_start = PRELUDE_LENGTH + PRELUDE_CRC_LENGTH;
        let headers_end = headers_start + headers_length;
        let headers_data = &self.buffer[headers_start..headers_end];
        let headers = parse_headers(headers_data)?;

        let payload_start = headers_end;
        let payload_end = message_crc_offset;
        let payload = self.buffer[payload_start..payload_end].to_vec();

        self.buffer.drain(0..total_length);

        let message_type = headers
            .iter()
            .find(|(k, _)| k == ":message-type")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();

        let content_type = headers
            .iter()
            .find(|(k, _)| k == ":content-type")
            .map(|(_, v)| v.clone());

        let event_type = headers
            .iter()
            .find(|(k, _)| k == ":event-type")
            .map(|(_, v)| v.clone());

        let exception_type = headers
            .iter()
            .find(|(k, _)| k == ":exception-type")
            .map(|(_, v)| v.clone());

        Ok(Some(AwsEvent {
            message_type,
            content_type,
            event_type,
            exception_type,
            payload,
        }))
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
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
    pub exception_type: Option<String>,
    pub payload: Vec<u8>,
}

impl AwsEvent {
    pub fn parse_json<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.payload)?)
    }

    pub fn is_event(&self) -> bool {
        self.message_type == "event"
    }

    pub fn is_exception(&self) -> bool {
        self.message_type == "exception"
    }
}

fn parse_headers(data: &[u8]) -> Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    let mut cursor = Cursor::new(data);

    while cursor.position() < data.len() as u64 {
        let name_length = read_u8(&mut cursor)? as usize;
        let name = read_string(&mut cursor, name_length)?;
        let value_type = read_u8(&mut cursor)?;

        let value = match value_type {
            HEADER_TYPE_BOOL_TRUE => "true".to_string(),
            HEADER_TYPE_BOOL_FALSE => "false".to_string(),
            HEADER_TYPE_BYTE => {
                let v = read_u8(&mut cursor)?;
                v.to_string()
            }
            HEADER_TYPE_SHORT => {
                let v = read_i16(&mut cursor)?;
                v.to_string()
            }
            HEADER_TYPE_INT => {
                let v = read_i32(&mut cursor)?;
                v.to_string()
            }
            HEADER_TYPE_LONG => {
                let v = read_i64(&mut cursor)?;
                v.to_string()
            }
            HEADER_TYPE_BYTES => {
                let len = read_u16(&mut cursor)? as usize;
                let mut buf = vec![0u8; len];
                cursor.read_exact(&mut buf)?;
                hex::encode(buf)
            }
            HEADER_TYPE_STRING => {
                let len = read_u16(&mut cursor)? as usize;
                read_string(&mut cursor, len)?
            }
            HEADER_TYPE_TIMESTAMP => {
                let v = read_i64(&mut cursor)?;
                v.to_string()
            }
            HEADER_TYPE_UUID => {
                let mut buf = [0u8; 16];
                cursor.read_exact(&mut buf)?;
                format!(
                    "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
                    u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]),
                    u16::from_be_bytes([buf[4], buf[5]]),
                    u16::from_be_bytes([buf[6], buf[7]]),
                    u16::from_be_bytes([buf[8], buf[9]]),
                    u64::from_be_bytes([0, 0, buf[10], buf[11], buf[12], buf[13], buf[14], buf[15]])
                )
            }
            _ => return Err(anyhow!("Unknown header type: {}", value_type)),
        };

        headers.push((name, value));
    }

    Ok(headers)
}

fn read_u8(cursor: &mut Cursor<&[u8]>) -> Result<u8> {
    let mut buf = [0u8; 1];
    cursor.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_u16(cursor: &mut Cursor<&[u8]>) -> Result<u16> {
    let mut buf = [0u8; 2];
    cursor.read_exact(&mut buf)?;
    Ok(u16::from_be_bytes(buf))
}

fn read_i16(cursor: &mut Cursor<&[u8]>) -> Result<i16> {
    let mut buf = [0u8; 2];
    cursor.read_exact(&mut buf)?;
    Ok(i16::from_be_bytes(buf))
}

fn read_i32(cursor: &mut Cursor<&[u8]>) -> Result<i32> {
    let mut buf = [0u8; 4];
    cursor.read_exact(&mut buf)?;
    Ok(i32::from_be_bytes(buf))
}

fn read_i64(cursor: &mut Cursor<&[u8]>) -> Result<i64> {
    let mut buf = [0u8; 8];
    cursor.read_exact(&mut buf)?;
    Ok(i64::from_be_bytes(buf))
}

fn read_string(cursor: &mut Cursor<&[u8]>, len: usize) -> Result<String> {
    let mut buf = vec![0u8; len];
    cursor.read_exact(&mut buf)?;
    Ok(String::from_utf8(buf)?)
}

fn crc32c(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0x82F63B78;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}
