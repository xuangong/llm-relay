# LLM Relay

多 Gateway 管理桌面应用 — 管理多个 [copilot-api-gateway](https://github.com/copilot-api-gateway) 实例，通过本地代理为 CLI 工具提供透明的故障转移和流量监控。

<div align="center">
  <img src="icon-source.svg" width="128" height="128" alt="LLM Relay Logo" />
</div>

## 特性

### 🔀 本地代理模式

LLM Relay 在本地启动一个 HTTP 代理（`127.0.0.1:18080`），CLI 工具的配置只需写入一次，指向代理地址。之后所有请求都经由代理转发，Gateway 切换对 CLI 完全透明，**无需重启**。

- **Claude Code** → `ANTHROPIC_BASE_URL=http://127.0.0.1:18080`
- **Codex CLI** → `base_url=http://127.0.0.1:18080/`
- **Gemini CLI** → `GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:18080`

### 🎯 核心功能

- **多 Gateway 管理** — 添加、编辑、删除多个 copilot-api-gateway 实例
- **健康监测** — 每 60 秒并发检查所有 Gateway 的健康状态、延迟和可用模型数
- **智能自动切换（Auto Failover）** — 按优先级自动切换到最佳健康 Gateway；当前 Gateway 仍健康时切换受 60 秒滞后保护，防止频繁抖动
- **代理流量触发切换** — 连续 3 次 5xx / 网络错误后立即切换，无需等待下次健康检查
- **拖拽排序** — 拖动调整 Gateway 优先级
- **CLI 自动配置** — 点击 "Use" 一次性写入所有 CLI 配置文件，之后 Gateway 切换不再修改文件

### 📊 流量监控

- **Token 统计** — 实时统计每个模型的 token 用量（input / output / cache），支持 Today / This Week / 7 Days / 30 Days 四个维度；自动解析 Anthropic SSE 流式响应和非流式 JSON
- **异常流量日志** — 记录所有 4xx / 5xx / 网络错误，保留 24 小时；显示状态码、延迟、路径、错误详情
- 底部面板可折叠，有新错误时显示角标

### 🤝 客户端身份

- 发送心跳到 Gateway，服务端可实时查看在线客户端（Connected Relays）
- 支持自定义客户端名称，显示格式：`name@hostname (IP)`

### ⚙️ 其他

- **系统托盘** — 快速切换 Gateway、查看状态
- **开机启动（Launch at Login）** — macOS LaunchAgent 注册
- 崩溃日志自动写入 `~/.llm-relay/crash.log`

## 安装

### 前置要求

- macOS（主要测试平台）/ Linux / Windows
- Node.js 18+
- Rust 工具链

### 快速开始

```bash
git clone <your-repo-url>
cd llm-relay
./setup.sh   # 安装 Rust、pnpm 及项目依赖
./dev.sh     # 启动开发服务器（热重载）
```

构建生产版本：

```bash
pnpm build
```

生成物在 `src-tauri/target/release/bundle/`。

## 使用方法

### 添加 Gateway

1. 点击 "Add Gateway"
2. 填写 **Name**、**URL**（如 `https://my-gateway.example.com`）、**Auth Key**
3. 点击 "Add" — 应用自动验证并检测健康状态

### 配置并启用

1. 展开 Gateway 卡片
2. 选择 **API Key**（从 Gateway 拉取）
3. 选择各 CLI 的模型（Claude / Claude Small / Codex / Gemini），无可用模型的分类会跳过
4. 点击 **Use** — 写入所有 CLI 配置文件，代理开始转发到该 Gateway

### 自动切换（Auto Failover）

右上角 **Auto Failover** 开关控制。开启后：

| 触发条件 | 行为 |
|---------|------|
| 当前 Gateway 下线（健康检查失败） | 立即切换到下一个健康 Gateway |
| 代理连续 3 次 5xx / 网络错误 | 立即切换 |
| 更高优先级 Gateway 恢复上线 | 60 秒滞后后切回 |

### 流量监控面板

底部面板点击展开：

- **Usage** — Token 用量统计，按模型分组，支持时间筛选和 Gateway 筛选
- **Errors** — 异常流量日志，按时间倒序，支持 Gateway 筛选；有新错误时显示红色角标

### 系统托盘

- 点击 Gateway 名称快速切换
- "Auto-switch: ON/OFF" 切换自动切换
- "Open Main Window" 打开主窗口

## 配置文件位置

| 文件 | 说明 |
|------|------|
| `~/.llm-relay/config.db` | SQLite 数据库（网关、配置、健康日志、流量日志、用量统计） |
| `~/.claude/settings.json` | Claude Code 配置（env 部分） |
| `~/.codex/config.toml` + `auth.json` | Codex CLI 配置 |
| `~/.gemini/.env` + `settings.json` | Gemini CLI 配置 |
| `~/.llm-relay/crash.log` | 崩溃日志 |

## 技术栈

| 层 | 技术 |
|----|------|
| 前端 | React 18 + TypeScript + Vite |
| UI | shadcn/ui + TailwindCSS + Framer Motion |
| 桌面框架 | Tauri 2 (Rust) |
| 数据库 | SQLite via rusqlite |
| 本地代理 | axum 0.8 |
| HTTP 客户端 | reqwest 0.12（含流式支持） |
| 异步运行时 | tokio |

## 项目结构

```
llm-relay/
├── src/
│   ├── components/
│   │   ├── GatewayCard.tsx      # 网关卡片（健康图、流量点、模型选择）
│   │   ├── GatewayList.tsx      # 拖拽排序列表
│   │   ├── UsagePanel.tsx       # Token 用量统计面板
│   │   └── TrafficLogPanel.tsx  # 异常流量日志面板
│   ├── lib/
│   │   ├── api.ts               # 所有 Tauri invoke 调用
│   │   └── error.ts             # 错误提取工具
│   └── App.tsx
├── src-tauri/src/
│   ├── proxy_server.rs          # 本地 HTTP 代理（axum，端口 18080）
│   ├── health.rs                # 健康检查循环 + 自动切换逻辑
│   ├── database.rs              # SQLite schema（v4 迁移）+ CRUD
│   ├── commands.rs              # Tauri 命令
│   ├── config_writer.rs         # CLI 配置文件读写
│   ├── gateway.rs               # Gateway API（健康检查、密钥、模型）
│   └── tray.rs                  # 系统托盘
├── setup.sh
└── dev.sh
```

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

## 常见问题

**Q: 点关闭按钮应用没有退出？**
正常行为 — 窗口最小化到托盘。在托盘菜单点 "Quit" 完全退出。

**Q: 代理不运行时 CLI 工具怎么办？**
CLI 配置指向 `127.0.0.1:18080`，如果应用未运行，请求会连接失败。建议开启 **Launch at Login** 保证代理始终在线。

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
