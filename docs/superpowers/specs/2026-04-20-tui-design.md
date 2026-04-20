# LLM Relay TUI 版本设计

**日期**: 2026-04-20
**状态**: Draft
**目标平台**: macOS / Linux / Windows（重点 Linux server，无桌面环境）

## 背景

当前 LLM Relay 是 Tauri 桌面应用，不能在无 GUI 的 Linux 服务器上运行。需要补一个 TUI 版本，提供与 GUI 同等的 gateway 管理、健康监控、流量监控、登录功能。

## 设计原则

- **TUI 与 GUI 共享核心逻辑**：抽出 core crate，避免双份维护
- **代理常驻**：TUI 关闭后代理子进程继续运行（fork + detach 模式）
- **GUI 与 TUI 互斥**：通过端口 18080 + lock 文件检测，启动时若另一方在跑则报错退出
- **Linux server 友好**：无 Secret Service / DBus 时自动 fallback 到加密文件存储

## 架构

将 `src-tauri/src` 拆为 cargo workspace 的多个 crate：

```
llm-relay/
├── crates/
│   ├── llm-relay-core/        # 与 UI 无关的逻辑（GUI 与 agent 共享）
│   │   ├── database.rs        # SQLite schema、CRUD
│   │   ├── gateway.rs         # gateway 健康检查 / key / model API / device login
│   │   ├── health.rs          # 健康循环 + auto failover
│   │   ├── proxy_server.rs    # axum 本地代理 (18080)
│   │   ├── config_writer.rs   # CLI 配置文件读写
│   │   ├── keystore.rs        # 系统 keychain + Linux 加密文件 fallback
│   │   └── ipc.rs             # IPC 协议定义（Request / Response / Event）
│   ├── llm-relay-agent/       # 常驻代理子进程
│   │   └── main.rs            # proxy + health loop + IPC server
│   └── llm-relay-tui/         # ratatui + crossterm
│       └── main.rs            # IPC client + UI
└── src-tauri/                 # 现有 Tauri GUI（依赖 llm-relay-core，不走 IPC）
```

**注意**：GUI 端**不**改造为 IPC 客户端，保持现有"内嵌代理"模式（启动时若 agent 在跑则报错），TUI 才走 fork agent + IPC 模式。

## 进程模型与生命周期

```
TUI 启动:
  1. 读 ~/.llm-relay/agent.pid
  2. 若 PID 存在且进程存活 → 连接 ~/.llm-relay/agent.sock，进入"附加模式"
  3. 否则检查 18080 端口
     - 被占用 → 报错退出（"port in use by PID xxx, probably GUI. Stop it first."）
     - 空闲 → fork llm-relay-agent 子进程并 detach
  4. 连接 agent.sock，进入 TUI 主循环

TUI 退出:
  - 仅断开 socket；agent 进程继续运行
  - 通过 'q' 菜单可选 "Quit & stop agent"

显式停止 agent:
  - llm-relay-tui --stop
  - 或 TUI 内 Settings tab → [stop agent]

GUI 启动:
  - 同样检测 agent.pid + 18080
  - agent 在跑 → 报错（提示先 stop agent）
  - 空闲 → 内嵌运行（保持现有行为）
```

**互斥保证**：跨平台文件锁 + 端口绑定双重检查。

文件锁用 [`fs2`](https://crates.io/crates/fs2) crate（封装 Unix `flock` / Windows `LockFileEx`），路径 `~/.llm-relay/agent.lock`。Agent 启动时尝试获取排他锁，失败则说明已有 agent 在跑（即使 PID 文件被意外删除也能检测到）。锁随进程退出自动释放。

### Daemon fork 实现

跨平台用 [`daemonize`](https://crates.io/crates/daemonize) (Unix) + Windows `CreateProcessW` with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`，包一层：

```rust
fn spawn_agent_detached() -> Result<u32 /* pid */> {
    let exe = std::env::current_exe()?.with_file_name("llm-relay-agent");
    // unix: daemonize → pid file, redirect stdio → ~/.llm-relay/agent.log
    // windows: CreateProcessW DETACHED_PROCESS
}
```

Agent 进程职责：
1. `flock(agent.lock)` 占据互斥
2. 写 PID 到 `agent.pid`
3. 启动 axum proxy server :18080
4. 启动 health check loop (60s)
5. 启动 IPC server 监听 `agent.sock`
6. 收到 `Shutdown` RPC 或 SIGTERM 时优雅退出（删 pid/sock 文件）

## IPC 协议

跨平台 local socket 用 [`interprocess`](https://crates.io/crates/interprocess) crate（Unix socket / Windows named pipe 同一 API）。

**帧格式**：4-byte 大端序长度前缀 + UTF-8 JSON body。每条消息独立解析，不混用 NDJSON。

**消息封装**：所有消息走统一 envelope，区分 RPC 响应与服务端推送事件，并用 `request_id` 关联请求/响应：

```rust
// crates/llm-relay-core/src/ipc.rs

/// 客户端 → 服务端
struct ClientFrame {
    request_id: u64,        // 客户端单调递增，事件订阅请求也带 id
    payload: Request,
}

/// 服务端 → 客户端：要么是某个请求的响应，要么是无请求的事件推送
enum ServerFrame {
    Response { request_id: u64, payload: Response },
    Event(Event),           // 无 request_id，独立通道
}

enum Request {
    GetSnapshot,
    Subscribe { topics: Vec<Topic> },
    Unsubscribe { topics: Vec<Topic> },
    AddGateway(GatewayInput),
    UpdateGateway { id: Uuid, /* ... */ },
    DeleteGateway { id: Uuid },
    SetActive { gateway_id: Uuid, key_id: Uuid, models: ModelSelection },
    SetAutoFailover(bool),
    Reorder(Vec<Uuid>),
    GetUsage { range: TimeRange, gateway_id: Option<Uuid> },
    GetTrafficLog { gateway_id: Option<Uuid> },
    StartLogin { gateway_id: Uuid },
    CancelLogin { gateway_id: Uuid },
    Shutdown,
}

enum Response {
    Snapshot(Snapshot),
    Ok,
    Error(String),
    LoginInitiated {
        device_code: String,    // 内部 polling 用
        user_code: String,      // 给用户输入的
        verification_url: String,
        expires_at: DateTime<Utc>,
        interval_secs: u64,     // 来自 gateway 响应
    },
}

enum Event {
    HealthChanged { gateway_id: Uuid, status: HealthStatus },
    ActiveChanged { gateway_id: Uuid },
    TrafficError { /* ... */ },
    UsageUpdate { /* ... */ },
    LoginCompleted { gateway_id: Uuid, user_name: Option<String> },
    LoginFailed { gateway_id: Uuid, reason: String },
    LoginExpired { gateway_id: Uuid },
}
```

客户端用 `request_id → oneshot::Sender<Response>` 的 map 来路由响应；`Event` 走独立的 `broadcast::Sender` 分发到 UI 层。

## TUI 界面布局

ratatui，四个 tab：Gateways / Usage / Errors / Settings。基础键位 `Tab` 切 tab、`?` 帮助、`q` 退出。

### Tab 1: Gateways

```
┌─ LLM Relay ──────────────────────── Auto Failover: [ON]  Agent: ●running ─┐
│  [Gateways] Usage  Errors  Settings                               18080 ●  │
├────────────────────────────────────────────────────────────────────────────┤
│  ▶ ● my-gateway-1     https://gw1.example.com    23ms   12 models    ★    │
│      key: prod-xxxxxx     claude: sonnet-4   codex: gpt-5  gemini: 2.5    │
│  ─────────────────────────────────────────────────────────────────────    │
│    ● my-gateway-2     https://gw2.example.com    87ms    8 models         │
│    ○ backup-gw        https://gw3.example.com   timeout                   │
├────────────────────────────────────────────────────────────────────────────┤
│ [a]dd  [e]dit  [d]el  [u]se  [k]ey  [m]odels  [l]ogin  [↑↓] reorder      │
└────────────────────────────────────────────────────────────────────────────┘
```

- `▶` 当前展开行；`★` 当前激活；`●/○` 健康状态
- `Enter` 展开/收起；`u` 激活该 gateway（写 CLI 配置）
- `k` 弹出 key 选择列表；`m` 弹出 model 选择（按 claude/codex/gemini 分类）
- `l` 启动登录流程
- `Shift+↑/↓` 重排优先级

### Tab 2: Usage

```
┌─ Usage ───────── Range: [Today] Week  7d  30d   Filter: [All gateways] ──┐
│ Model                          Input        Output       Cache    Total  │
├──────────────────────────────────────────────────────────────────────────┤
│ claude-sonnet-4-5            120,432       45,221      2.1M       2.3M  │
│ gpt-5                         34,201       12,883          -    47,084  │
│ gemini-2.5-pro                 8,902        3,201          -    12,103  │
└──────────────────────────────────────────────────────────────────────────┘
```

- `1/2/3/4` 切时间范围；`g` 切 gateway 过滤
- 不画图（YAGNI），数字表格够用

### Tab 3: Errors

```
┌─ Traffic Errors (last 24h) ────────── 12 errors    Filter: [All] ───────┐
│ Time      GW           Status  Path                  ms   Detail        │
├──────────────────────────────────────────────────────────────────────────┤
│ 14:23:01  my-gateway-1  503    /v1/messages         812  upstream down  │
│ 14:21:55  my-gateway-1  502    /v1/messages         203  bad gateway    │
└──────────────────────────────────────────────────────────────────────────┘
```

`Enter` 查看完整错误详情弹窗。

### Tab 4: Settings

```
┌─ Settings ───────────────────────────────────────────────────────────────┐
│  Client name:        [my-laptop                  ]                       │
│  Auto failover:      [x] enabled                                         │
│  Launch at login:    [x] enabled (systemd user unit)                     │
│                                                                          │
│  Agent:                                                                  │
│    PID file:   ~/.llm-relay/agent.pid (12345)                           │
│    Socket:     ~/.llm-relay/agent.sock                                  │
│    Uptime:     2h 14m                                                    │
│    [stop agent]   [restart agent]                                        │
│                                                                          │
│  Storage:                                                                │
│    DB:         ~/.llm-relay/config.db (1.2 MB)                          │
│    Secrets:    keychain (Linux: encrypted file)                         │
└──────────────────────────────────────────────────────────────────────────┘
```

### 弹窗：Add Gateway

```
            ┌─ Add Gateway ──────────────────┐
            │  Name: [_________________]     │
            │  URL:  [_________________]     │
            │  Auth: [_________________]     │
            │       [Cancel]  [Add]          │
            └────────────────────────────────┘
```

### 弹窗：Login（OAuth Device Flow）

复用现有 gateway device code 端点：
- `POST <gateway>/auth/device/code` → 获取 `device_code` / `user_code` / `expires_in` / `interval`
- `POST <gateway>/auth/device/poll` (body `{ "device_code": ... }`) → 状态轮询
- 用户在浏览器打开 `<gateway>/device/login` 输入 `user_code` 完成验证

```
┌─ Login to my-gateway-1 ────────────────────────────┐
│                                                     │
│  Open this URL on any device:                       │
│                                                     │
│    https://gw1.example.com/device/login             │
│                                                     │
│  Enter code:                                        │
│                                                     │
│    ┌─────────────────┐                              │
│    │   ABCD-1234     │                              │
│    └─────────────────┘                              │
│                                                     │
│  Waiting for verification... (expires in 9:42)      │
│                                                     │
│  [c] copy URL    [Esc] cancel                       │
└─────────────────────────────────────────────────────┘
```

流程：
1. 用户在 TUI 选 gateway → 按 `l`
2. TUI 发送 `StartLogin { gateway_id }` 给 agent
3. Agent 调 `POST /auth/device/code`，回 `LoginInitiated { device_code, user_code, verification_url, expires_at, interval_secs }`
4. TUI 弹窗展示 `verification_url` + `user_code`
5. Agent 后台按服务端返回的 `interval_secs` 间隔轮询 `POST /auth/device/poll`（不硬编码 5s）
6. 用户在手机/另一台电脑打开 URL、输入 code、登录
7. Agent 收到 `status=complete` → 保存 session token 到 keystore → 推送 `LoginCompleted`
8. TUI 弹窗自动关闭

### 状态栏

底部一行始终显示：`active gateway · health · proxy port · agent PID · last error`

## Keychain / 密钥存储

`llm-relay-core/src/keystore.rs` 改造为优先尝试系统 keychain，失败时 fallback 到加密文件（不靠环境变量启发式判断）：

```rust
enum KeyBackend {
    SystemKeychain,   // macOS Keychain / Windows Credential Manager / Linux Secret Service
    EncryptedFile,    // ~/.llm-relay/secrets.enc，AES-256-GCM
}

fn open_backend() -> KeyBackend {
    // 先尝试系统 keychain（一次实际探测：写读删一个 sentinel 项）
    match try_init_system_keychain() {
        Ok(()) => KeyBackend::SystemKeychain,
        Err(e) => {
            tracing::warn!("system keychain unavailable, falling back to encrypted file: {e}");
            KeyBackend::EncryptedFile
        }
    }
}
```

加密文件方案：
- 主密码来源：优先 `LLM_RELAY_KEY` 环境变量；否则首次运行 prompt（TUI 弹窗输入）
- 用 argon2 派生 AES-256 key
- 密码缓存在 agent 进程内存，agent 重启需重新输入或配 env

## 错误处理

| 场景 | 行为 |
|------|------|
| TUI 启动时 18080 被占用，且非 agent 进程 | 报错退出："port 18080 in use by PID xxx (probably GUI). Stop it first." |
| TUI 启动时 agent.pid 存在但进程已死 | 删除 stale pid，正常 fork |
| TUI 连接 agent.sock 失败 | 重试 3 次后报错 |
| Agent 内部 panic | 写 `~/.llm-relay/crash.log`，退出；TUI 检测连接断开后提示重启 |
| keychain 不可用（Linux server） | 自动 fallback 加密文件，TUI 顶部提示 "secrets in encrypted file" |
| device login 超时 | 推送 `LoginExpired`，TUI 弹窗提示重试 |

## 测试策略

| 层 | 测试 |
|----|------|
| `llm-relay-core` | 单元测试（已有的从 src-tauri 迁过来），SQLite 用临时 DB |
| IPC 协议 | round-trip 测试：encode/decode 所有 Request / Response / Event；request_id 关联正确性 |
| `llm-relay-agent` | 集成测试：启动 agent → IPC client 调 AddGateway → 查 SQLite 验证 |
| `llm-relay-tui` | 渲染测试：mock IPC client，对每个 view snapshot ratatui buffer |
| 跨平台 spawn | macOS / Linux / Windows CI 各跑一次 spawn → ping → shutdown 冒烟 |
| 生命周期边界 | (1) stale agent.sock 残留时启动 agent；(2) PID 文件存在但 PID 已被其他无关进程占用；(3) 两个 TUI 同时启动竞争 lock；(4) agent 进程 kill -9 后 lock/pid 残留的恢复路径 |

## 不做（YAGNI）

- 远程 TUI（通过网络连别人的 agent）
- agent 自动重启 / 看门狗（用 systemd 处理）
- TUI 主题切换、鼠标支持
- Token 用量图表（数字表格够用，TUI 画图体验差）
- GUI 改造为 IPC 客户端（现有内嵌代理模式继续保留）

## 实施顺序

1. 提取 `llm-relay-core` crate（database / gateway / health / proxy_server / config_writer / keystore），src-tauri 改为依赖它
2. 定义 IPC 协议 + 实现 server / client（`interprocess` + `serde_json`）
3. 实现 `llm-relay-agent` bin（含 daemonize + pidfile + socket + lock）
4. 实现 `llm-relay-tui` bin（ratatui，先做 Gateways tab + Settings tab）
5. 补 Login 弹窗 + Usage / Errors tab
6. Linux 加密文件 keystore fallback
7. systemd user unit 模板 + 文档
8. CI：三平台 spawn 冒烟测试

## 配置文件 / 路径

| 路径 | 说明 |
|------|------|
| `~/.llm-relay/agent.pid` | 代理子进程 PID 文件 |
| `~/.llm-relay/agent.sock` | IPC socket（Windows: `\\.\pipe\llm-relay-agent`） |
| `~/.llm-relay/agent.lock` | flock 互斥文件 |
| `~/.llm-relay/agent.log` | agent stdout/stderr 重定向 |
| `~/.llm-relay/secrets.enc` | Linux server fallback 密钥文件 |
| `~/.llm-relay/config.db` | SQLite（沿用现有） |
| `~/.llm-relay/crash.log` | 崩溃日志（沿用现有） |
