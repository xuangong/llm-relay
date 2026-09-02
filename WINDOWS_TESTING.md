# Windows 测试清单

本文档用于在 Windows 平台上首次测试 LLM Relay 时验证功能。

## 构建 Windows 安装包

### 在 Windows 机器上构建

```powershell
# 安装依赖
npm install -g pnpm
pnpm install

# 构建
pnpm run tauri build
```

构建产物位置：
- NSIS 安装器：`src-tauri\target\release\bundle\nsis\LLM Relay_0.4.1_x64-setup.exe`
- MSI 安装器：`src-tauri\target\release\bundle\msi\LLM Relay_0.4.1_x64_en-US.msi`

### 交叉编译（从 macOS/Linux）

```bash
# 安装 Windows 目标
rustup target add x86_64-pc-windows-msvc

# 构建（需要 Windows SDK）
pnpm run tauri build --target x86_64-pc-windows-msvc
```

## 测试清单

### ✅ 基础功能

- [ ] 应用启动成功
- [ ] 系统托盘图标显示正常
- [ ] 主窗口 UI 正常渲染
- [ ] 点击关闭按钮，窗口隐藏到托盘（不退出）
- [ ] 托盘菜单可以重新打开窗口
- [ ] 托盘菜单 "Quit" 可以完全退出

### ✅ 数据库功能

- [ ] 添加 Gateway 成功
- [ ] 编辑 Gateway 成功
- [ ] 删除 Gateway 成功
- [ ] 拖拽排序功能正常
- [ ] 应用重启后数据保留

**数据库位置**：`C:\Users\<username>\.llm-relay\config.db`

### ✅ 健康检查

- [ ] 手动点击 "Check Health" 按钮有响应
- [ ] 健康状态正确显示（绿色/红色）
- [ ] 延迟显示正常
- [ ] 健康历史图表显示正常

### ✅ 本地代理

- [ ] 代理服务器启动（`127.0.0.1:18080`）
- [ ] 测试代理可访问：
  ```powershell
  curl http://127.0.0.1:18080/health
  ```
- [ ] 流量监控 "Usage" 面板显示正常
- [ ] 流量监控 "Errors" 面板显示正常
- [ ] 流量点指示器更新正常

### ✅ 自动切换

- [ ] "Auto Failover" 开关可以切换
- [ ] 手动点击 "Use" 成功切换 Gateway
- [ ] 当前 Gateway 下线时自动切换（需要等待健康检查）
- [ ] 系统托盘显示当前 Gateway 名称

### ✅ 开机启动

- [ ] "Launch at Login" 开关可以切换
- [ ] 开启后，重启 Windows 应用自动启动
- [ ] 关闭后，重启 Windows 应用不启动

**注册表检查**：
```powershell
# 应该看到 LLM Relay 条目
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Run"
```

### ⚠️ CLI 配置路径验证（关键）

这是 **最关键的测试**！需要验证 CLI 工具在 Windows 上的实际配置路径。

#### 1. 安装 CLI 工具

按照各工具的官方文档在 Windows 上安装：
- Claude Code
- Codex CLI（如果有 Windows 版本）
- Gemini CLI（如果有 Windows 版本）

#### 2. 运行一次生成配置文件

```powershell
# 运行各个 CLI，让它们生成默认配置
claude --version
codex --version
gemini --version
```

#### 3. 查找配置文件位置

使用 Windows 搜索或命令行查找：

```powershell
# 方法 1: 使用 dir 递归查找
dir /s /b C:\Users\%USERNAME%\.claude 2>nul
dir /s /b C:\Users\%USERNAME%\.codex 2>nul
dir /s /b C:\Users\%USERNAME%\.gemini 2>nul

# 方法 2: 查找 AppData 目录
dir /s /b C:\Users\%USERNAME%\AppData\*claude* 2>nul
dir /s /b C:\Users\%USERNAME%\AppData\*codex* 2>nul
dir /s /b C:\Users\%USERNAME%\AppData\*gemini* 2>nul

# 方法 3: 使用 PowerShell
Get-ChildItem -Path "$env:USERPROFILE" -Recurse -Filter "*.claude*" -ErrorAction SilentlyContinue
Get-ChildItem -Path "$env:APPDATA" -Recurse -Filter "*settings.json" -ErrorAction SilentlyContinue
```

#### 4. 记录实际路径

创建一个测试报告，记录：

| CLI 工具 | 预期路径（代码中） | 实际路径（Windows 上） | 是否匹配 |
|---------|-------------------|---------------------|---------|
| Claude Code | `C:\Users\<user>\.claude\settings.json` | ? | ? |
| Codex CLI | `C:\Users\<user>\.codex\config.toml` | ? | ? |
| Gemini CLI | `C:\Users\<user>\.gemini\.env` | ? | ? |

#### 5. 测试配置写入

在 LLM Relay 中：
1. 添加一个 Gateway
2. 选择 API Key 和模型
3. 点击 "Use"

然后检查：
```powershell
# 检查 Claude Code 配置
type C:\Users\%USERNAME%\.claude\settings.json

# 检查 Codex 配置
type C:\Users\%USERNAME%\.codex\config.toml
type C:\Users\%USERNAME%\.codex\auth.json

# 检查 Gemini 配置
type C:\Users\%USERNAME%\.gemini\.env
type C:\Users\%USERNAME%\.gemini\settings.json
```

验证：
- [ ] 配置文件成功创建/更新
- [ ] `ANTHROPIC_BASE_URL` / `base_url` / `GEMINI_API_BASE_URL` + `GOOGLE_GEMINI_BASE_URL` 指向 `http://127.0.0.1:18080`（Gemini 同时写入两个变量名以兼容新旧 CLI）
- [ ] API Key 设置为 `llm-relay-local`（placeholder）

#### 6. 测试 CLI 工具实际使用

```powershell
# 测试 Claude Code 是否能通过代理工作
claude "test prompt"

# 观察 LLM Relay 的 Usage 面板是否记录了流量
```

### 📝 如果路径不匹配

如果发现 CLI 工具在 Windows 上使用的配置路径与代码中不同（例如使用 `%APPDATA%\Claude` 而不是 `%USERPROFILE%\.claude`），请：

1. 记录所有实际路径
2. 在 GitHub Issue 中报告
3. 需要修改 `src-tauri/src/config_writer.rs`，添加 Windows 特定路径逻辑

## 已知限制

- **Reopen 事件**：Windows 没有 macOS Dock 的 "Reopen" 概念，点击托盘图标打开窗口即可
- **文件权限**：Windows 不使用 Unix 权限模式，`atomic_write` 已经跳过权限保存逻辑

## 报告问题

如果发现任何问题，请在 GitHub Issues 中报告，包含：
- Windows 版本（Windows 10/11）
- 错误截图或日志（`C:\Users\<username>\.llm-relay\crash.log`）
- CLI 配置路径的实际位置（如果与预期不同）

## 构建时常见问题

### 缺少 Windows SDK

如果构建时报错缺少 `windows.h` 等头文件：
1. 安装 Visual Studio 2022（选择 "Desktop development with C++"）
2. 或安装 Windows SDK

### Rust 工具链

```powershell
# 安装 Rust
winget install Rustlang.Rustup

# 或使用官方安装器
# https://rustup.rs/
```
