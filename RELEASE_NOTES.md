# LLM Relay v0.3.3

## 新增

### 🔌 一键停用中继 + 自动恢复原始 CLI 配置
- Header 新增 "Disable Relay" 按钮（仅在已激活时显示）
- 首次启用中继时自动快照 `~/.claude/settings.json`、`~/.codex/config.toml` + `auth.json`、`~/.gemini/.env` + `settings.json` 里被覆盖的字段
- 停用时弹出确认框，逐条列出"将恢复为 X"或"将删除（原本未设置）"
- 有快照：恢复每个字段到原值（Codex `[model_providers.copilot_gateway]` 整段还原）
- 无快照：等价于旧的 clear 行为，仅清除中继写入的字段，CLI 回退默认（如官方订阅接口）
- 快照存放在 `~/.llm-relay/cli-config-backup.json`，停用成功后自动删除

之前停用中继得手动改回每个 CLI 的配置文件——现在一键解决。

## 下载

| 平台 | 文件 |
|------|------|
| macOS Universal | `LLM Relay_0.3.3_universal.dmg` |
| Windows GUI | `LLM Relay_0.3.3_x64-setup.exe` |
| TUI/agent Linux x64 | `llm-relay-agent-x86_64-unknown-linux-gnu`, `llm-relay-tui-x86_64-unknown-linux-gnu` |
| TUI/agent macOS Apple Silicon | `llm-relay-agent-aarch64-apple-darwin`, `llm-relay-tui-aarch64-apple-darwin` |
| TUI/agent macOS Intel | `llm-relay-agent-x86_64-apple-darwin`, `llm-relay-tui-x86_64-apple-darwin` |
