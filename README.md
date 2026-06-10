# LLM Relay

多 Gateway 管理桌面应用 — 管理多个 [copilot-api-gateway](https://github.com/copilot-api-gateway) 实例，通过本地代理为 CLI 工具提供透明的故障转移和流量监控。

<div align="center">
  <img src="icon-source.svg" width="128" height="128" alt="LLM Relay Logo" />
</div>

> **v0.3.3 新增**：TUI（终端界面）+ 无头 agent 守护进程，可在服务器上长期驻留；GUI 与 TUI 自动互斥，GUI 检测到守护进程时支持一键接管。

---

## 架构速览

```
             ┌──────────────────────────────────────────────┐
   CLI 工具  │  Claude Code / Codex / Gemini CLI            │
 (一次性配置)│  统一指向 http://127.0.0.1:18080             │
             └─────────────────┬────────────────────────────┘
                               │ HTTP
                               ▼
             ┌──────────────────────────────────────────────┐
             │  LLM Relay 本地代理 (axum, 18080)            │
             │  健康检查 · 自动故障转移 · 流量/用量记录    │
             └─────────────────┬────────────────────────────┘
                               │ reqwest
                               ▼
             ┌──────────┐ ┌──────────┐ ┌──────────┐
             │ Gateway A│ │ Gateway B│ │ Gateway C│  ...
             └──────────┘ └──────────┘ └──────────┘

   两种运行模式（同一进程，互斥启动）：

   ┌────────────────┐          ┌────────────────────────────┐
   │   GUI 桌面版   │   或     │   无头 agent + TUI 客户端  │
   │ (Tauri window) │          │ (后台 daemon + ratatui)    │
   └────────────────┘          └────────────────────────────┘
           └────────── 共享 ~/.llm-relay/agent.lock ──────────┘
                         （谁先启动谁占用 18080）
```

---

## 特性

### 🔀 本地代理模式

LLM Relay 在本地启动一个 HTTP 代理（`127.0.0.1:18080`），CLI 工具的配置只需写入一次，指向代理地址。之后所有请求都经由代理转发，Gateway 切换对 CLI **完全透明，无需重启**。

- **Claude Code** → `ANTHROPIC_BASE_URL=http://127.0.0.1:18080`
- **Codex CLI** → `base_url=http://127.0.0.1:18080/`
- **Gemini CLI** → `GEMINI_API_BASE_URL` + `GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:18080`（同时写入两个变量名以兼容新旧 Gemini CLI）

### 🎯 核心功能

- **多 Gateway 管理** — 添加、编辑、删除多个 copilot-api-gateway 实例
- **健康监测** — 每 60 秒并发检查所有 Gateway 的健康状态、延迟和可用模型数
- **智能自动切换（Auto Failover）** — 按优先级自动切换到最佳健康 Gateway；当前 Gateway 仍健康时切换受 60 秒滞后保护，防止频繁抖动
- **代理流量触发切换** — 连续 10 次 5xx / 网络错误后立即切换，无需等待下次健康检查
- **拖拽排序** — 拖动调整 Gateway 优先级
- **CLI 自动配置** — 点击 "Use" 一次性写入所有 CLI 配置文件，之后 Gateway 切换不再修改文件

### 💻 TUI & 无头 agent（v0.3.3）

- **独立 agent 进程**：`llm-relay-agent` 作为后台守护进程运行，GUI / TUI 都是它的客户端
- **TUI 客户端**：`llm-relay-tui` 基于 ratatui，可在纯终端环境（服务器、SSH、tmux）下完整管理 Gateway
- **GUI ↔ agent 互斥**：基于共享的 `~/.llm-relay/agent.lock` 文件锁 + 原子端口绑定，避免 TOCTOU
- **GUI 一键接管**：GUI 检测到 agent 守护进程时，弹窗提示 “停止守护进程并启动 GUI”，点击 Yes 自动关停 agent、重新占用端口、打开主窗口（GUI > TUI 优先级）
- **加密环境变量模式**：服务器部署无 OS keychain 时，agent 用 `LLM_RELAY_MASTER_KEY`（AES-256-GCM 的 32 字节主密钥，base64）加密存储 secrets

### 📊 流量监控

- **Token 统计** — 实时统计每个模型的 token 用量（input / output / cache），支持 Today / This Week / 7 Days / 30 Days 四个维度；自动解析 Anthropic SSE 流式响应和非流式 JSON
- **异常流量日志** — 记录所有 4xx / 5xx / 网络错误，保留 24 小时；显示状态码、延迟、路径、错误详情
- 底部面板可折叠，有新错误时显示角标

### 🤝 客户端身份

- 发送心跳到 Gateway，服务端可实时查看在线客户端（Connected Relays）
- 支持自定义客户端名称，显示格式：`name@hostname (IP)`

### ⚙️ 其他

- **系统托盘** — 快速切换 Gateway、查看状态
- **开机启动（Launch at Login）** — 跨平台支持（macOS LaunchAgent / Windows Startup / Linux Systemd）
- 崩溃日志自动写入 `~/.llm-relay/crash.log`

---

## 安装

### 下载预编译包

从 [Releases](https://github.com/xuangong/llm-relay/releases) 页面下载适合你平台的安装包：

- **macOS GUI**: `LLM Relay_0.3.3_universal.dmg`（Universal，支持 Apple Silicon + Intel）
- **Windows GUI**: `LLM Relay_0.3.3_x64-setup.exe`（NSIS 安装器）
- **纯 TUI / 服务器部署**：按平台下载下面列出的 agent + TUI 二进制：
  - Linux x64: `llm-relay-agent-x86_64-unknown-linux-gnu` + `llm-relay-tui-x86_64-unknown-linux-gnu`
  - macOS Apple Silicon: `llm-relay-agent-aarch64-apple-darwin` + `llm-relay-tui-aarch64-apple-darwin`
  - macOS Intel: `llm-relay-agent-x86_64-apple-darwin` + `llm-relay-tui-x86_64-apple-darwin`
  - systemd 部署见 [packaging/systemd/](packaging/systemd/)

### 从源码构建

详细的构建说明请查看 **[BUILD.md](./BUILD.md)**。

```bash
git clone https://github.com/xuangong/llm-relay.git
cd llm-relay
./setup.sh   # 安装 Rust、pnpm 及项目依赖
./dev.sh     # 启动开发服务器（热重载）

# 构建 GUI 安装包
pnpm build

# 只构建 TUI + agent（无 GUI 依赖）
cargo build --release -p llm-relay-tui -p llm-relay-agent
```

生成物：
- GUI: `src-tauri/target/release/bundle/`
- TUI/agent: `target/release/llm-relay-tui`、`target/release/llm-relay-agent`

---

## 快速上手（GUI）

### 1. 添加第一个 Gateway

```
点击 "Add Gateway"
  ├─ Name:      my-gateway
  ├─ URL:       https://my-gateway.example.com
  └─ Auth Key:  sk-...
点击 "Add" → 自动验证 + 健康检查
```

### 2. 激活 Gateway

```
展开卡片
  ├─ API Key:   [下拉选择]                 ← 从 Gateway 拉取
  ├─ Claude:    claude-sonnet-4-6         ← 为各 CLI 分别选模型
  ├─ Codex:     gpt-5
  └─ Gemini:    gemini-2.5-pro
点击 "Use" → 写入 CLI 配置文件
```

### 3. CLI 工具用起来

配置文件只写一次，以后换 Gateway 不影响 CLI：

```bash
# Claude Code：~/.claude/settings.json 已写入 ANTHROPIC_BASE_URL
claude "hello"

# Codex：~/.codex/config.toml 已写入 base_url
codex

# Gemini：~/.gemini/.env 已写入 GEMINI_API_BASE_URL + GOOGLE_GEMINI_BASE_URL（兼容新旧 CLI）
gemini
```

### 4. 开启自动故障转移

右上角 **Auto Failover** 开关 → ON

| 触发条件 | 行为 |
|---------|------|
| 当前 Gateway 下线（健康检查失败） | 立即切换到下一个健康 Gateway |
| 代理连续 10 次 5xx / 网络错误 | 立即切换 |
| 更高优先级 Gateway 恢复上线 | 60 秒滞后后切回 |

### 5. 查看用量 / 错误

底部面板点击展开：

- **Usage** — Token 用量按模型分组，支持时间 / Gateway 筛选
- **Errors** — 异常流量按时间倒序，红色角标提示新错误

---

## 服务器部署（TUI + 无头 agent）

没有 GUI 环境、想长期在服务器跑？用无头 agent：

### 1. 准备主密钥（服务器模式必需）

```bash
# 生成 32 字节 base64 主密钥（仅需一次，写入服务器的 env/secret manager）
openssl rand -base64 32

export LLM_RELAY_MASTER_KEY="生成的 base64 字符串"
```

> agent 用这个密钥 AES-256-GCM 加密 `~/.llm-relay/secrets.env.enc`。没有这个环境变量 agent 拒绝启动。

### 2. 启动 agent（后台）

```bash
# 手动
./llm-relay-agent &

# systemd user service（推荐）
mkdir -p ~/.config/systemd/user
cp packaging/systemd/llm-relay-agent.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now llm-relay-agent.service
sudo loginctl enable-linger "$USER"
```

### 3. 连上 TUI

```bash
./llm-relay-tui
```

首次启动 TUI 会自动 spawn 一个 detached agent（如果还没有），之后只作为客户端连接 IPC socket（Unix socket / Windows named pipe）。

### TUI 快捷键

| 键 | 动作 |
|----|------|
| `Tab` / `Shift+Tab` | 切换 Gateways / Usage / Errors / Settings 标签 |
| `↑` / `↓` | 选择行 |
| `Enter` | 展开 / 编辑 |
| `a` | 添加 Gateway |
| `e` | 编辑 |
| `l` | Login（device auth flow） |
| `s` | 星标置顶 |
| `r` | 手动刷新健康 |
| `q` | 退出 TUI（agent 继续运行） |

---

## 在 WSL2 中使用（Windows）

LLM Relay 在 Windows GUI 模式下会自动管理 WSL2 里的 Claude / Codex / Gemini CLI 配置。

### 自动检测

启动 Relay 后在主界面下方的 **WSL2 Distros** 面板：

- 自动列出所有已安装的 WSL2 distro（WSL1 会被过滤）
- 默认 distro 默认勾选；其它 distro 可手动勾选
- 每行显示该 distro 是否已装 claude / codex / gemini —— 没装的会跳过

### 网络

Relay 在 Windows 的 `127.0.0.1:18080` 和 WSL 虚拟网卡 IP（通常 `172.x.x.1:18080`）上各开一个监听 ——
**物理网卡（以太网 / WiFi）完全不监听，局域网无法触及**。

> **首次启动 Windows Defender 防火墙提示**：Windows 会弹一次"是否允许 LLM Relay 接收网络连接"。
> **必须选 Allow**，否则 WSL 端连不上 `172.x.x.1:18080`。如果一开始误点了 Block，去
> 「Windows 安全中心 → 防火墙和网络保护 → 允许应用通过防火墙」改成允许专用网络，或者删掉那条
> 阻止规则后重启 Relay 让它再弹一次。

WSL 端写入 CLI 配置的 URL 按下面优先级挑：

1. `http://127.0.0.1:18080` —— mirror 网络模式下成立（distro loopback 直接是 host loopback），最稳。
2. `http://host.docker.internal:18080` —— NAT 模式下，且 `host.docker.internal` 没被劫持时成立。
3. `http://llm-relay-18080.host:18080` —— Relay 在 distro 的 `/etc/hosts` 里注入一条
   `<gateway_ip> llm-relay-18080.host`，避开 Docker Desktop 把 `host.docker.internal` 解析到
   局域网 IP 的常见情况。WSL 重启 / `wsl --shutdown` 后 gateway IP 变了，state machine 会自动
   重写这一行；CLI 配置文件本身永远不变，跑着的 claude/codex/gemini 进程不用重启。

注入 `/etc/hosts` 用 `wsl.exe -d <distro> -u root`，**不会弹 sudo 密码**（host 已经隐式信任 distro，
WSL 的设计如此）。卸载 / 取消勾选时这一行会被删除。

启动时 Relay 会从 distro 内部跑一次 HTTP probe，挑出可达的 URL 写入配置。
要求 distro 里装了 `curl` 或 `wget`（极简镜像如裸 Alpine 可能没装；UI 会提示 Unreachable）。

### 取消勾选 / Disable

- 取消勾选某 distro → 该 distro 的 CLI 配置恢复到首次 apply 之前的状态
- Disable Relay（清空 active gateway）→ Windows + 所有勾选 distro 都恢复
- 重装 / unregister 一个 distro 之前请先取消勾选，否则恢复快照会失败（数据无损，只是 warning 进 log）

---

## GUI ↔ TUI 互斥与接管

GUI 和 agent 使用同一把锁（`~/.llm-relay/agent.lock`）+ 同一个端口（18080），**同时只能存在一个**。

```
情景 A：GUI 先启动
  → agent 启动时锁获取失败 → 立即退出，stderr 打印 "already running"

情景 B：agent 先启动（例如 systemd 起的守护进程）
  → GUI 启动时弹窗：
        ┌────────────────────────────────────────────┐
        │  LLM Relay                                 │
        │  ─────────────────────────────             │
        │  The LLM Relay daemon (PID 12345)          │
        │  is already running.                       │
        │                                            │
        │  Stop the daemon and launch the GUI        │
        │  instead?              [ Yes ]  [ No ]     │
        └────────────────────────────────────────────┘
  点 Yes → GUI 通过 IPC 发送 Shutdown → 等待锁释放（最多 5s）
         → 重新 acquire → 打开主窗口
  点 No  → GUI 静默退出
```

这样设计的好处：**GUI 优先级总是大于 TUI**，用户不会被一个忘了关的守护进程永久锁在外面。

命令行关 agent 也一样可用：

```bash
# 优雅停（发 Shutdown IPC）
kill -TERM $(cat ~/.llm-relay/agent.pid)

# 或直接 kill；agent 会自动清理 pidfile / socket
```

---

## 配置文件位置

| 文件 | 说明 |
|------|------|
| `~/.llm-relay/config.db` | SQLite 数据库（网关、配置、健康日志、流量日志、用量统计） |
| `~/.llm-relay/agent.lock` | GUI/agent 互斥文件锁 |
| `~/.llm-relay/agent.pid` | 当前运行进程的 PID |
| `~/.llm-relay/agent.sock` | IPC socket（Unix）/ 命名管道（Windows） |
| `~/.llm-relay/secrets.enc` | GUI 模式下的加密 secrets（用 OS keychain 主密钥派生） |
| `~/.llm-relay/secrets.env.enc` | 无头模式下的加密 secrets（用 `LLM_RELAY_MASTER_KEY`） |
| `~/.claude/settings.json` | Claude Code 配置（env 部分） |
| `~/.codex/config.toml` + `auth.json` | Codex CLI 配置 |
| `~/.gemini/.env` + `settings.json` | Gemini CLI 配置 |
| `~/.llm-relay/crash.log` | 崩溃日志 |

---

## 环境变量

| 变量 | 作用 | 默认 |
|------|------|------|
| `LLM_RELAY_MASTER_KEY` | 无头 agent 的 AES 主密钥（base64 32B） | 必填（headless） |
| `LLM_RELAY_PROXY_PORT` | 覆盖代理端口（测试 / 多实例） | `18080` |
| `LLM_RELAY_RUNTIME_DIR` | 覆盖 runtime 目录（锁 / pid / socket） | `~/.llm-relay/` |

---

## 技术栈

| 层 | 技术 |
|----|------|
| 前端 | React 18 + TypeScript + Vite |
| UI | shadcn/ui + TailwindCSS + Framer Motion |
| 桌面框架 | Tauri 2 (Rust) |
| TUI | ratatui + crossterm |
| 数据库 | SQLite via rusqlite |
| 本地代理 | axum 0.8 |
| IPC | interprocess 2（Unix socket / Windows named pipe）+ 自制长度前缀 JSON 帧 |
| 加密 | aes-gcm 0.10（server 模式）、argon2 0.5（GUI 模式） |
| HTTP 客户端 | reqwest 0.12（含流式支持） |
| 异步运行时 | tokio |
| 进程锁 | fs2 |

---

## 项目结构

```
llm-relay/
├── src/                          # React 前端
│   ├── components/
│   ├── lib/
│   └── App.tsx
├── src-tauri/                    # Tauri GUI (shell only — 业务在 core)
│   └── src/lib.rs                # GUI 生命周期 + daemon 接管弹窗
├── crates/
│   ├── llm-relay-core/           # 共享核心：DB、proxy、health、IPC、keystore
│   │   ├── src/lifecycle.rs      # GUI/agent 共享的锁 + 端口绑定 + 接管 helper
│   │   ├── src/keystore/         # system / encrypted_file / env 三种后端
│   │   ├── src/proxy_server.rs
│   │   └── src/ipc/              # 长度前缀 JSON 协议
│   ├── llm-relay-agent/          # 无头 agent 二进制
│   └── llm-relay-tui/            # ratatui 客户端
├── packaging/systemd/            # Linux 服务单元
├── setup.sh
└── dev.sh
```

---

## 数据库 Schema

```
gateways          — 网关列表（id, name, url, auth_key, sort_order, ...）
active_config     — 当前激活配置（gateway_id, key, models, auto_switch, ...）
health_cache      — 最新健康状态缓存（per gateway）
health_check_log  — 历史健康检查记录（最近 1440 条/网关）
traffic_log       — 异常流量日志（4xx/5xx，保留 24h）
usage_log         — Token 用量（gateway × model × hour 聚合）
settings          — 键值配置（client_name, client_id）
```

---

## 常见问题

**Q: 点关闭按钮应用没有退出？**
正常行为 — 窗口最小化到托盘。在托盘菜单点 "Quit" 完全退出。

**Q: 代理不运行时 CLI 工具怎么办？**
CLI 配置指向 `127.0.0.1:18080`，如果应用未运行，请求会连接失败。建议开启 **Launch at Login**，或用 systemd 跑无头 agent，保证代理始终在线。

**Q: 同时在 GUI 和服务器 agent 之间切换，会丢配置吗？**
不会。两者共享同一个 `~/.llm-relay/config.db`，只是运行时互斥。GUI 的"一键停止 daemon"只是让端口让出来，数据完好。

**Q: 服务器上 `keyring` 报错怎么办？**
用无头模式（`llm-relay-agent`）+ `LLM_RELAY_MASTER_KEY`。此模式完全跳过 OS keychain，所有 secrets 加密写到磁盘文件。

**Q: Health Monitor 图是空的？**
点击 "Use" 启用 Gateway 后，60 秒内会完成第一次健康检查并开始填充图表。

**Q: Token 统计没有数据？**
仅统计经过代理的请求。确认 `~/.claude/settings.json` 中 `ANTHROPIC_BASE_URL` 为 `http://127.0.0.1:18080`，且 `ANTHROPIC_AUTH_TOKEN` 为 `llm-relay-local`。

**Q: 健康检查频率是多少？**
每 60 秒一次，所有 Gateway 并发检查。

---

**提示**: 如遇 `cargo` 命令找不到，先加载 Rust 环境：

```bash
source ~/.cargo/env
```
