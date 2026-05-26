# LLM Relay v0.3.1

## 重点更新

### 🆕 TUI + 无头 agent（服务器模式）
- 新增 `llm-relay-agent` 独立二进制 —— 后台守护进程，可通过 systemd 常驻服务器
- 新增 `llm-relay-tui` 终端客户端（ratatui）—— 纯终端环境下完整管理 Gateway，SSH / tmux 都能用
- agent + TUI 走 IPC（Unix socket / Windows named pipe，长度前缀 JSON 帧）通信

### 🔒 GUI ↔ TUI 互斥 + 一键接管
- GUI 和 agent 共享 `~/.llm-relay/agent.lock` 文件锁 + 原子端口绑定，永远只能跑一个
- GUI 启动时如果检测到 agent 守护进程，会弹窗：**「停止守护进程并启动 GUI？」**
  - 点 Yes → 自动 IPC 发 Shutdown → 等锁释放 → 打开 GUI
  - 点 No → 静默退出
- 这样设计让 GUI 永远优先于 TUI，避免用户被忘关的守护进程永久锁在外面

### 🔐 加密环境变量模式
- 无头部署（服务器）没有 OS keychain，新增 `LLM_RELAY_MASTER_KEY` 模式
- 用 AES-256-GCM + base64 32 字节主密钥加密 secrets 到 `~/.llm-relay/secrets.env.enc`
- agent 启动时要求这个环境变量，否则拒绝启动

### 🧪 新增环境变量
- `LLM_RELAY_MASTER_KEY` —— 无头模式的 AES 主密钥（base64 32B）
- `LLM_RELAY_PROXY_PORT` —— 覆盖代理端口（默认 18080），便于测试 / 多实例
- `LLM_RELAY_RUNTIME_DIR` —— 覆盖 runtime 目录（锁 / pid / socket）

### 📖 文档
- README 完全重写：架构示意图、GUI 快速上手、服务器 / TUI 部署指南、接管流程图
- BUILD.md 新增：WSL2 构建 Linux 版本、手动本地构建 + 上传发布流程、TUI / agent 构建

## 测试
- `pnpm typecheck`、`pnpm build:renderer`、`pnpm check:release-version` 全绿
- `cargo test --workspace` 全绿（41 个非 ignored 测试）
- `cargo test -p llm-relay-agent --test lifecycle_integration -- --ignored --test-threads=1` 全绿（4 个集成测试）
- `cargo test -p llm-relay-agent --test mutual_exclusion -- --ignored --test-threads=1` 全绿（1 个集成测试）
- `pnpm tauri build --target universal-apple-darwin` 成功生成 macOS Universal DMG

## 下载

| 平台 | 文件 |
|------|------|
| macOS Universal | `LLM Relay_0.3.1_universal.dmg` |
| Windows GUI | `LLM Relay_0.3.1_x64-setup.exe` |
| TUI/agent Linux x64 | `llm-relay-agent-x86_64-unknown-linux-gnu`, `llm-relay-tui-x86_64-unknown-linux-gnu` |
| TUI/agent macOS Apple Silicon | `llm-relay-agent-aarch64-apple-darwin`, `llm-relay-tui-aarch64-apple-darwin` |
| TUI/agent macOS Intel | `llm-relay-agent-x86_64-apple-darwin`, `llm-relay-tui-x86_64-apple-darwin` |

> ⚠️ 本版本的部分包由本地手动构建 + 上传（CI 临时不可用）。未签名 — macOS 首次打开时右键选 "打开" 绕过 Gatekeeper。
