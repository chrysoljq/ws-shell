use chrono::Utc;
use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use tokio::sync::mpsc;
use ws_shell_common::{ClientInfo, ServerMsg};

const LOG_BUFFER_SIZE: usize = 200;

/// One connected client
pub struct ConnectedClient {
    pub info: ClientInfo,
    pub tx: mpsc::UnboundedSender<String>, // send ServerMsg JSON to client
}

pub struct AppState {
    pub clients: DashMap<String, ConnectedClient>,
    /// WebUI WebSocket listeners — broadcast client output to all
    pub webui_listeners: DashMap<String, mpsc::UnboundedSender<String>>,
    /// Recent log buffer for replaying to new WebUI connections
    pub log_buffer: Mutex<VecDeque<String>>,
    pub jwt_secret: String,
    pub admin_username: String,
    pub admin_password_hash: String,
}

impl AppState {
    pub fn add_client(
        &self,
        id: String,
        hostname: String,
        os: String,
        arch: String,
        addr: String,
        tx: mpsc::UnboundedSender<String>,
    ) {
        let info = ClientInfo {
            id: id.clone(),
            hostname,
            os,
            arch,
            connected_at: Utc::now().to_rfc3339(),
            last_heartbeat: None,
            addr,
        };
        self.clients.insert(id, ConnectedClient { info, tx });
    }

    pub fn remove_client(&self, id: &str) {
        self.clients.remove(id);
    }

    pub fn get_client(&self, id: &str) -> Option<ClientInfo> {
        self.clients.get(id).map(|c| c.info.clone())
    }

    /// Force-disconnect a client by dropping its sender channel.
    pub fn disconnect_client(&self, id: &str) -> bool {
        self.clients.remove(id).is_some()
    }

    pub fn list_clients(&self) -> Vec<ClientInfo> {
        self.clients.iter().map(|c| c.info.clone()).collect()
    }

    pub fn send_to_client(&self, client_id: &str, msg: &ServerMsg) -> Result<(), String> {
        if let Some(client) = self.clients.get(client_id) {
            let json = serde_json::to_string(msg).map_err(|e| e.to_string())?;
            client.tx.send(json).map_err(|e| e.to_string())
        } else {
            Err(format!("Client {} not found", client_id))
        }
    }

    /// Store a log message in the buffer and broadcast to all WebUI listeners
    pub fn broadcast_to_webui(&self, msg: &str) {
        // Store in buffer
        if let Ok(mut buf) = self.log_buffer.lock() {
            buf.push_back(msg.to_string());
            while buf.len() > LOG_BUFFER_SIZE {
                buf.pop_front();
            }
        }

        // Broadcast to live listeners, collecting dead ones
        let mut dead = Vec::new();
        for entry in self.webui_listeners.iter() {
            if entry.value().send(msg.to_string()).is_err() {
                dead.push(entry.key().clone());
            }
        }
        for id in dead {
            self.webui_listeners.remove(&id);
        }
    }

    /// Get recent log messages for replaying to a new WebUI connection
    pub fn get_log_history(&self) -> Vec<String> {
        self.log_buffer
            .lock()
            .map(|buf| buf.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn add_webui_listener(&self, id: String, tx: mpsc::UnboundedSender<String>) {
        self.webui_listeners.insert(id, tx);
    }

    pub fn remove_webui_listener(&self, id: &str) {
        self.webui_listeners.remove(id);
    }
}
