# LLM Relay v0.3.2

## 修复

### 🐛 第二次启动 GUI 不再误报 "AlreadyRunning"
- 把 lifecycle guard（文件锁 + 端口绑定）的获取从 `tauri::Builder` 之前挪到 `.setup()` 内部
- 之前 guard 抢在 `tauri_plugin_single_instance` 之前执行，重复启动 GUI 会弹错误对话框
- 现在由 single-instance 插件先静默 focus 已有窗口；guard 失败只在 GUI vs 守护进程的真实冲突时触发，daemon-takeover 对话框才有意义

## 下载

| 平台 | 文件 |
|------|------|
| macOS Universal | `LLM Relay_0.3.2_universal.dmg` |
| Windows GUI | `LLM Relay_0.3.2_x64-setup.exe` |
| TUI/agent Linux x64 | `llm-relay-agent-x86_64-unknown-linux-gnu`, `llm-relay-tui-x86_64-unknown-linux-gnu` |
| TUI/agent macOS Apple Silicon | `llm-relay-agent-aarch64-apple-darwin`, `llm-relay-tui-aarch64-apple-darwin` |
| TUI/agent macOS Intel | `llm-relay-agent-x86_64-apple-darwin`, `llm-relay-tui-x86_64-apple-darwin` |

完整 v0.3.x 功能说明见 [v0.3.0 release notes](https://github.com/xuangong/llm-relay/releases/tag/v0.3.0)。
