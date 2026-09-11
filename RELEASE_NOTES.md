# LLM Relay v0.5.0

## 新增

### ⚙️ 设置侧边抽屉
- Header 右端新增 `≡`，点开右侧抽屉
- 收进抽屉：设备名称、语言、开机启动、自动故障转移、WSL2 发行版、停用中继
- 主界面只保留盯着网关列表时真正会用的：使用指南、健康检查刷新、抽屉入口
- 自动故障转移**关闭时**，header 显示琥珀色提示 chip —— 回答"为什么故障网关还挂着"
- WSL2 发行版从主列表移入抽屉，不再是常驻的视觉干扰；顺带补齐了它缺失的中文翻译

### 🖥️ 关闭窗口后不留痕
- macOS：关闭主窗口后 Dock 图标消失，只保留菜单栏图标
- Windows：任务栏无图标，托盘常驻；**单击托盘打开窗口，右击出菜单**
- 托盘 "Open Main Window"、macOS Dock 重新激活、第二实例启动，都会正确把窗口带到最前

### 🔧 写入的配置现在真的能启动起来
- Claude Code：补上 `~/.claude.json` 的 `hasCompletedOnboarding`，否则它会走首次运行向导、压根不读我们写的 settings.json（已存在则不写，文件损坏则原样保留）
- Codex CLI：`OPENAI_API_KEY` 统一写 `llm-relay-ignore`，按当前 `$SHELL` 决定写哪个文件与语法（`.zshrc` / macOS `.bash_profile` / `.bashrc` / `config.fish` / `.profile`），已有赋值就地替换而非追加
- 你自己设置的真实 key 不会被动到——代理本来就会在链路上替换它
- host 与每个 WSL 发行版分别处理（WSL 的登录 shell 读 passwd，而非 `$SHELL`）

### 🔇 错误日志按路径屏蔽
- 悬停某行即可屏蔽该路径（例如 Claude Code 的 `/api/hello` 预检探活，在不提供该路由的网关上必然 404）
- 屏蔽在 SQL 层过滤，条数上限只统计可见行——高频噪音不再把真实错误挤出视野
- Errors 徽标同样跳过已屏蔽路径

### 🆔 心跳上报 OS 机器标识
- 修掉网关面板上同一台机器堆成好几行的问题
- 额外上报 macOS `IOPlatformUUID` / Windows `MachineGuid` / Linux `/etc/machine-id`，只在重装系统时才变
- `clientId` 仍照发以便服务端合并历史；读不到时该字段整个省略，不发占位值

### 其他
- 网关行内重命名与置顶
- 点击网关地址在系统浏览器中打开
- TUI 首次运行主密钥向导

## 修复

- **备份缺失不再阻止同步**：本地删除原始备份后，Use / Save / WSL 自动重试仍可继续。恢复时找不到原来存在的备份，就保留当前配置，不删除文件或把代理配置重新记录为原始备份；停用仍关闭本地代理并释放端口。

- **停用中继释放本地代理端口**：Disable Relay 先关闭本机及 WSL 监听、断开正在转发的连接，再恢复 CLI 配置；恢复失败也保持停止。停用状态跨重启保留，后台健康检查及 WSL 自动重试不会重新启用。再次 Use 时先绑定端口，端口被其他程序占用则返回错误。停用入口移到主窗口，并显示实际代理运行状态和端口占用。
- **本地备份允许自由修改**：取消 `.llm-relay.origin` 和 `.llm-relay.bak` 的 SHA-256 记录与校验，旧清单中的哈希字段会被忽略。修改备份内容不会阻止 Use / Save / WSL 同步；恢复时直接使用备份当前的完整内容。停用时先把工作配置保存为 `.bak`，再恢复 `.origin`；下次 Use 先把当时的工作配置保存为 `.origin`，再恢复已有 `.bak`。清单仍区分原文件不存在与空文件；备份缺失时保留当前配置，实际读取错误仍会报错。
- **Use / Save 在备份缺失时自动重建配置**：数据库仍显示启用、但恢复记录缺失时，有 `.bak` 就恢复后生成，没有则从空配置生成，避免保存被阻止；离线 WSL 重连后也会继续重建。
- **所选 key 在会话不可见时的回退**：`/api/keys` 按调用方裁剪结果，不拥有任何 key 的会话返回 `200 []`。此前这会写入 NULL 的 `key_value`，代理随后静默改用网关自身凭证转发——配额算错、归属算错、毫无报错。现在依次尝试各个凭证，且宁可报错也不写 NULL
- **代理正确读取压缩的上游响应体**，错误详情按字符（而非字节）截断，不再乱码
- **无 model 的请求不再标记 `[model:unknown]`**——无请求体的 GET 探测本就不存在 model
- **从未命名过的客户端也能起名**：旧版的名字编辑器在名字为空时宽度为 0，点不着；现在改名统一在设置抽屉里

## 下载

| 平台 | 文件 |
|------|------|
| macOS Universal | `LLM Relay_0.5.0_universal.dmg` |
| Windows GUI | `LLM Relay_0.5.0_x64-setup.exe` |
| TUI/agent Linux x64 | `llm-relay-agent-x86_64-unknown-linux-gnu`, `llm-relay-tui-x86_64-unknown-linux-gnu` |
| TUI/agent macOS Apple Silicon | `llm-relay-agent-aarch64-apple-darwin`, `llm-relay-tui-aarch64-apple-darwin` |
| TUI/agent macOS Intel | `llm-relay-agent-x86_64-apple-darwin`, `llm-relay-tui-x86_64-apple-darwin` |

> macOS 首次打开：dmg 未做 Apple 公证（仅 ad-hoc 签名），Gatekeeper 会提示"已损坏"或"无法验证开发者"。拖入 `/Applications` 后执行一次：
>
> ```sh
> xattr -dr com.apple.quarantine "/Applications/LLM Relay.app"
> ```
