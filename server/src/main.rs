use dashmap::DashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tracing::info;

mod auth;
mod state;
mod ws;

use state::AppState;

#[derive(clap::Parser)]
#[command(name = "ws-shell-server", about = "WebSocket remote shell server")]
struct Args {
    /// Listen port
    #[arg(short, long, default_value = "9888")]
    port: u16,

    /// JWT secret for authentication
    #[arg(short, long, default_value = "change-me-in-production")]
    secret: String,

    /// Admin username
    #[arg(long, default_value = "admin")]
    username: String,

    /// Admin password
    #[arg(long, default_value = "admin")]
    password: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ws_shell_server=info,tower_http=info".into()),
        )
        .init();

    let args = <Args as clap::Parser>::parse();
    let password_hash = bcrypt::hash(&args.password, 4).expect("bcrypt hash failed");

    let state = Arc::new(AppState {
        clients: DashMap::new(),
        webui_listeners: DashMap::new(),
        log_buffer: Mutex::new(VecDeque::new()),
        tasks: DashMap::new(),
        jwt_secret: args.secret.clone(),
        admin_username: args.username.clone(),
        admin_password_hash: password_hash,
    });

    let app = ws::build_router(state.clone());

    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));
    info!("Server listening on {}", addr);
    info!("WebUI: http://localhost:{}", args.port);
    info!("WS endpoint: ws://localhost:{}/ws/client", args.port);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
