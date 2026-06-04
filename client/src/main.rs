use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};
use ws_shell_common::{ClientMsg, ServerMsg, StreamType};

const SERVICE_VERSION: &str = "1.5.0";

#[derive(clap::Parser)]
#[command(name = "ws-shell-client", about = "WebSocket remote shell client")]
struct Args {
    /// Server WebSocket endpoint
    #[arg(short, long, default_value = "ws://127.0.0.1:9888/ws/client")]
    server: String,

    /// Authentication token (must match server --secret)
    #[arg(short, long, default_value = "change-me-in-production")]
    token: String,

    /// Heartbeat interval in seconds
    #[arg(long, default_value = "30")]
    heartbeat: u64,
}

/// Tracks running child processes so we can kill them on demand.
type TaskMap = Arc<RwLock<HashMap<String, tokio::process::Child>>>;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ws_shell_client=info".into()),
        )
        .init();

    let args = <Args as clap::Parser>::parse();

    info!("ws-shell-client v{} starting", SERVICE_VERSION);
    info!("server: {}", args.server);

    loop {
        info!("connecting to server...");
        match connect_and_run(&args).await {
            Ok(()) => info!("session ended"),
            Err(e) => error!("connection error: {}", e),
        }
        info!("reconnecting in 5s...");
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn connect_and_run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}?token={}", args.server, args.token);
    let (ws_stream, _) = connect_async(&url).await?;
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // 运行中的子进程表
    let tasks: TaskMap = Arc::new(RwLock::new(HashMap::new()));

    // 发送协程
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // 注册
    let hostname = gethostname::gethostname().to_string_lossy().to_string();
    let register = ClientMsg::Register {
        hostname,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        client_version: SERVICE_VERSION.to_string(),
    };
    tx.send(serde_json::to_string(&register)?)?;
    info!("registered with server");

    // 心跳
    let start_time = std::time::Instant::now();
    let heartbeat_interval = std::time::Duration::from_secs(args.heartbeat);
    let tx_hb = tx.clone();
    let hb_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(heartbeat_interval).await;
            let hostname = gethostname::gethostname().to_string_lossy().to_string();
            let hb = ClientMsg::Heartbeat {
                hostname,
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                uptime_secs: start_time.elapsed().as_secs(),
            };
            match serde_json::to_string(&hb) {
                Ok(json) => {
                    if tx_hb.send(json).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    warn!("failed to serialize heartbeat: {}", e);
                }
            }
        }
    });

    // 主循环
    while let Some(Ok(msg)) = ws_receiver.next().await {
        match msg {
            Message::Text(text) => {
                let text: &str = &text;
                match serde_json::from_str::<ServerMsg>(text) {
                    Ok(ServerMsg::Exec { id, cmd, cwd, env }) => {
                        info!("[task:{}] dispatching: {}", id, cmd);
                        let tx_cmd = tx.clone();
                        let tasks_ref = Arc::clone(&tasks);
                        tokio::spawn(async move {
                            run_task(tx_cmd, tasks_ref, &id, &cmd, cwd, env).await;
                        });
                    }
                    Ok(ServerMsg::Kill { id }) => {
                        info!("[task:{}] kill requested", id);
                        let mut map = tasks.write().await;
                        if let Some(mut child) = map.remove(&id) {
                            if let Err(e) = child.kill().await {
                                warn!("[task:{}] kill failed: {}", id, e);
                            } else {
                                info!("[task:{}] killed", id);
                            }
                        } else {
                            warn!("[task:{}] not found (already exited?)", id);
                        }
                    }
                    Ok(ServerMsg::Resize { .. }) => {} // PTY resize — 暂不支持
                    Err(e) => warn!("invalid message: {}", e),
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    hb_handle.abort();
    send_task.abort();
    Ok(())
}

async fn run_task(
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    tasks: TaskMap,
    id: &str,
    cmd: &str,
    cwd: Option<String>,
    env: Option<std::collections::HashMap<String, String>>,
) {
    let id = id.to_string();
    let mut command = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    };

    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    if let Some(vars) = env {
        for (k, v) in vars {
            command.env(k, v);
        }
    }

    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    // 确保子进程在被 kill 时能正确终止
    command.kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            error!("[task:{}] spawn failed: {}", id, e);
            let msg = ClientMsg::Error {
                id: id.clone(),
                msg: e.to_string(),
            };
            let _ = tx.send(serde_json::to_string(&msg).unwrap());
            return;
        }
    };

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    // 注册到 task map（供 Kill 使用）
    tasks.write().await.insert(id.clone(), child);

    // stdout
    let tx_out = tx.clone();
    let id_out = id.clone();
    let stdout_handle = tokio::spawn(async move {
        stream_output(stdout, tx_out, &id_out, StreamType::Stdout).await;
    });

    // stderr
    let tx_err = tx.clone();
    let id_err = id.clone();
    let stderr_handle = tokio::spawn(async move {
        stream_output(stderr, tx_err, &id_err, StreamType::Stderr).await;
    });

    let _ = tokio::join!(stdout_handle, stderr_handle);

    // 等待退出码 — 从 task map 取出 child
    let exit_code = if let Some(mut child) = tasks.write().await.remove(&id) {
        child.wait().await.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1)
    } else {
        // 已被 Kill 移除
        -1
    };

    let exit_msg = ClientMsg::Exit {
        id: id.clone(),
        code: exit_code,
    };
    let _ = tx.send(serde_json::to_string(&exit_msg).unwrap());

    info!("[task:{}] completed (exit {})", id, exit_code);
}

/// 通用的 stdout/stderr 流式读取，使用 lossy UTF-8 避免丢失非 UTF-8 数据。
async fn stream_output(
    mut reader: impl AsyncReadExt + Unpin,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    id: &str,
    stream: StreamType,
) {
    let mut buf = vec![0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                let msg = ClientMsg::Output {
                    id: id.to_string(),
                    data: text,
                    stream,
                };
                if tx.send(serde_json::to_string(&msg).unwrap()).is_err() {
                    break; // channel closed
                }
            }
            Err(e) => {
                warn!("[task:{}] {:?} read error: {}", id, stream, e);
                break;
            }
        }
    }
}
