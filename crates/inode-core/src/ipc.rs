//! IPC contract: newline-delimited JSON-RPC 2.0 over a Unix domain socket.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub const PROTOCOL_VERSION: &str = "2.0";

pub fn socket_path(uid: u32) -> PathBuf {
    let dir = if cfg!(target_os = "macos") {
        PathBuf::from("/var/run/inode-vpn")
    } else {
        PathBuf::from("/run/inode-vpn")
    };
    dir.join(uid.to_string()).join("daemon.sock")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, method: &str) -> Self {
        Self {
            jsonrpc: PROTOCOL_VERSION.into(),
            id,
            method: method.into(),
            params: Value::Null,
        }
    }

    pub fn with_params(id: u64, method: &str, params: Value) -> Self {
        Self {
            jsonrpc: PROTOCOL_VERSION.into(),
            id,
            method: method.into(),
            params,
        }
    }
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            jsonrpc: PROTOCOL_VERSION.into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: PROTOCOL_VERSION.into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

impl Event {
    pub fn new(method: &str, params: Value) -> Self {
        Self {
            jsonrpc: PROTOCOL_VERSION.into(),
            method: method.into(),
            params,
        }
    }
}

pub fn write_line(stream: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    let mut line =
        serde_json::to_vec(value).map_err(|e| Error::Ipc(format!("serialize failed: {e}")))?;
    line.push(b'\n');
    stream
        .write_all(&line)
        .map_err(|e| Error::Ipc(e.to_string()))
}

pub fn read_line(reader: &mut BufReader<UnixStream>) -> Result<String> {
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .map_err(|e| Error::Ipc(e.to_string()))?;
    if n == 0 {
        return Err(Error::Ipc("connection closed".into()));
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_platform_shape() {
        let p = socket_path(501);
        assert!(p.ends_with("501/daemon.sock"));
        assert!(p.starts_with("/run") || p.starts_with("/var/run"));
    }

    #[test]
    fn rpc_round_trip() {
        let req = Request::with_params(1, "start", serde_json::json!({}));
        let s = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&s).unwrap();
        assert_eq!(back.method, "start");
        assert_eq!(back.id, 1);
    }
}
