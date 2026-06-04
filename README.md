# WS Remote Shell

基于 WebSocket 的远程 Shell 控制系统。管理员通过 WebUI 连接远程机器，发送命令并实时接收输出。

## 项目结构

```
rust-ws-shell/
├── Cargo.toml           # Workspace 根配置
├── .cargo/config.toml   # 默认构建目标 x86_64-unknown-linux-musl
├── common/              # 共享协议类型库
│   └── src/lib.rs
├── server/              # 服务端（Axum HTTP + WebSocket）
│   └── src/
│       ├── main.rs      # 入口，CLI 参数解析
│       ├── auth.rs      # JWT 认证 + bcrypt 密码校验
│       ├── state.rs     # 应用状态管理（DashMap）
│       ├── ws.rs        # 路由、WebSocket 处理、REST API
│       └── static/index.html  # WebUI 前端
└── client/              # 客户端 Agent
    └── src/
        └── main.rs      # WebSocket 连接、命令执行、心跳上报
```

## 技术栈

| 组件 | 技术 |
|------|------|
| 语言 | Rust 2021 |
| 异步运行时 | Tokio |
| HTTP 框架 | Axum 0.8 |
| WebSocket | tokio-tungstenite（客户端）/ Axum WS（服务端） |
| 认证 | JWT (jsonwebtoken) + bcrypt |
| 序列化 | serde / serde_json |
| 并发数据结构 | dashmap |
| CLI 解析 | clap 4 |
| 日志 | tracing + tracing-subscriber |

## 快速开始

### 直接下载（推荐）

从 [Releases](https://github.com/chrysoljq/ws-shell/releases/tag/v0.1.0) 页面下载预编译的静态二进制，无需安装 Rust 环境：

```bash
# 下载服务端
curl -LO https://github.com/chrysoljq/ws-shell/releases/download/v0.1.0/server-linux-amd64.gz
gunzip server-linux-amd64.gz
chmod +x server-linux-amd64

# 下载客户端 Agent
curl -LO https://github.com/chrysoljq/ws-shell/releases/download/v0.1.0/client-linux-amd64.gz
gunzip client-linux-amd64.gz
chmod +x client-linux-amd64
```

### 从源码构建

```bash
# 构建服务端
cargo build --release -p ws-shell-server

# 构建客户端 Agent
cargo build --release -p ws-shell-client
```

Release 构建默认开启 `strip`、`LTO`、`opt-level = "z"`，生成体积最小的静态二进制。

### 启动服务端

```bash
# 直接下载的二进制
./server-linux-amd64 \
  --port 9888 \
  --jwt-secret your-secret-key \
  --admin-user admin \
  --admin-password your-password

# 或从源码构建的二进制
./target/x86_64-unknown-linux-musl/release/server \
  --port 9888 \
  --jwt-secret your-secret-key \
  --admin-user admin \
  --admin-password your-password
```

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--port` | 9888 | 监听端口 |
| `--jwt-secret` | change-me-in-production | JWT 签名密钥 |
| `--admin-user` | admin | 管理员用户名 |
| `--admin-password` | admin | 管理员密码 |

### 启动客户端 Agent

```bash
# 直接下载的二进制
./client-linux-amd64 \
  --server ws://YOUR_SERVER:9888 \
  --token YOUR_JWT_TOKEN \
  --heartbeat 30

# 或从源码构建的二进制
./target/x86_64-unknown-linux-musl/release/node-metrics-agent \
  --server ws://YOUR_SERVER:9888 \
  --token YOUR_JWT_TOKEN \
  --heartbeat 30
```

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--server` | - | 服务端 WebSocket 地址 |
| `--token` | - | 认证 Token |
| `--heartbeat` | 30 | 心跳间隔（秒） |

Agent 启动后会自动注册并保持长连接，断线每 5 秒自动重连。

### 访问 WebUI

浏览器打开 `http://YOUR_SERVER:9888`，使用管理员账号登录即可看到已连接的客户端列表和实时终端输出。

## API 接口

### 公开接口

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/` | WebUI 页面 |
| POST | `/api/v1/login` | 登录，返回 JWT |
| GET | `/api/v1/agent/latest` | 下载 Agent 二进制（gzip） |
| POST | `/api/v1/log` | Agent 日志上传 |

### 需认证接口（`Authorization: Bearer <token>`）

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/clients` | 列出所有已连接客户端 |
| GET | `/api/v1/clients/{id}` | 获取客户端信息 |
| DELETE | `/api/v1/clients/{id}` | 断开指定客户端 |
| POST | `/api/v1/clients/{id}/exec` | 向客户端发送命令 |
| POST | `/api/v1/clients/{id}/kill/{task_id}` | 终止运行中的任务 |

### WebSocket 端点

| 端点 | 说明 |
|------|------|
| `GET /ws?token=<jwt>` | WebUI 实时输出流 |
| `GET /ws/client?token=<jwt>` | Agent 连接端点 |

## 通信协议

服务端与客户端通过 JSON 消息通信，类型定义在 `common/src/lib.rs`。

**服务端 → 客户端（ServerMsg）：**
- `Exec { id, cmd, cwd, env }` — 执行命令
- `Kill { id }` — 终止任务
- `Resize { id, cols, rows }` — 终端尺寸调整（预留）

**客户端 → 服务端（ClientMsg）：**
- `Register { hostname, os, arch, client_version }` — 注册
- `Heartbeat { hostname, os, arch, uptime_secs }` — 心跳
- `Output { id, data, stream }` — 命令输出（stdout/stderr）
- `Exit { id, code }` — 命令退出
- `Error { id, msg }` — 执行错误

## 许可证

MIT
