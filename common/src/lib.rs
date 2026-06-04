use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Server → Client ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    /// Execute a command
    Exec {
        id: String,
        cmd: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        env: Option<HashMap<String, String>>,
    },
    /// Resize the PTY (future use)
    Resize { id: String, cols: u16, rows: u16 },
    /// Kill a running command
    Kill { id: String },
}

// ── Client → Server ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    /// Command output (stdout or stderr)
    Output {
        id: String,
        data: String,
        stream: StreamType,
    },
    /// Command exited
    Exit { id: String, code: i32 },
    /// Execution error (e.g. command not found)
    Error { id: String, msg: String },
    /// Client heartbeat with system info
    Heartbeat {
        hostname: String,
        os: String,
        arch: String,
        uptime_secs: u64,
    },
    /// Initial registration after connect
    Register {
        hostname: String,
        os: String,
        arch: String,
        client_version: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StreamType {
    Stdout,
    Stderr,
}

// ── REST API types ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub id: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub connected_at: String,
    pub last_heartbeat: Option<String>,
    pub addr: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecRequest {
    pub cmd: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecResponse {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageResponse {
    pub message: String,
}
