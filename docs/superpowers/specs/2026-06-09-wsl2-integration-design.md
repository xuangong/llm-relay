# WSL2 集成设计

**日期**：2026-06-09
**状态**：Spec (待用户确认)
**适用范围**：Windows-only 增量功能；mac/Linux 行为不变。

---

## 1. 背景

LLM Relay 当前作为 Windows 桌面应用运行：

- 本地代理监听 `127.0.0.1:18080`
- CLI 配置写入 Windows 用户目录（`C:\Users\<user>\.claude\`、`.codex\`、`.gemini\`）

用户也在同一台机器的 WSL2 里跑 Claude Code / Codex / Gemini CLI。WSL2 场景下当前架构两个问题都不解决：

1. **网络层**：WSL2 默认 NAT 模式下，`127.0.0.1` 是 WSL2 自己的 loopback，够不到 Windows 主机
2. **文件层**：WSL2 里的 CLI 读 Linux 路径 `/home/<user>/.claude/...`，Windows 端写的配置完全不可见

本设计在不影响 Windows 原生使用、不破坏 mac/Linux 平台的前提下，让 WSL2 里跑的 CLI 工具也能透明使用 LLM Relay。

---

## 2. 设计要点

### 2.1 网络层：精准 bind + `host.docker.internal`

Relay 同时 bind 两个具体接口（**不**用 `0.0.0.0`，避免 LAN 暴露和防火墙弹窗）：

- `127.0.0.1:18080` — Windows 本机访问
- `<WSL2 网关 IP>:18080` — WSL2 通过虚拟网卡访问

WSL2 网关 IP 是 Windows 上虚拟网卡 `vEthernet (WSL)` 的 IPv4，通常形如 `172.x.x.1`，**每次 Windows / WSL 重启会变**。

写入 CLI 配置的 URL：

| Target | URL |
|---|---|
| Windows | `http://127.0.0.1:18080` |
| WSL2 | `http://host.docker.internal:18080` |

`host.docker.internal` 由 WSL 0.65+ 自动注入 `/etc/hosts`，指向 WSL2 网关 IP。在 NAT 和 mirror 两种模式下都解析正确，所以 WSL 端不需要按模式切 URL。

#### 网关 IP 发现与重 bind
- 启动时：用 Windows IP Helper API（如 `ipconfig` crate）枚举 adapter，找名字含 "WSL" 的，取它的 IPv4
- 探测不到（用户没装 WSL / WSL 网卡未启用）→ 只 bind `127.0.0.1`，跳过 WSL 相关功能
- 周期性检测：每 60 秒（健康检查 tick 上搭车）重新探测；IP 变化 → 重 bind 新 IP，drop 旧 listener

#### 物理网卡上完全没有 listener
- 同 WiFi/办公网的别人 SYN 不会被接受 → 0 LAN 暴露
- 不需要任何防火墙规则、不需要 token

### 2.2 Distro 管理：用户多选，默认勾选默认 distro

#### 发现
- `wsl.exe -l -v --quiet` 列所有已注册 distro + 运行状态
- `wsl.exe --status` 拿默认 distro 名

#### Probe（每个 distro 一次性 + 手动刷新触发）
对每个 distro 拉取：
- `home` ← `wsl -d <D> -e sh -c 'echo $HOME'`
- `user` ← `wsl -d <D> -e whoami`
- `has_claude` / `has_codex` / `has_gemini` ← `wsl -d <D> -e sh -c 'command -v claude && command -v codex && command -v gemini'`（一次 shell 调用拿三个结果）

结果写入 SQLite。新增表：

```sql
CREATE TABLE wsl_distros (
  name          TEXT PRIMARY KEY,
  is_default    INTEGER NOT NULL DEFAULT 0,
  selected      INTEGER NOT NULL DEFAULT 0,  -- 用户勾选
  home          TEXT,                         -- probe 缓存
  user          TEXT,
  has_claude    INTEGER NOT NULL DEFAULT 0,
  has_codex     INTEGER NOT NULL DEFAULT 0,
  has_gemini    INTEGER NOT NULL DEFAULT 0,
  probed_at     TEXT
);
```

#### UI（Settings 页面）
Windows 平台下新增一块 "WSL2 Distros"：

```
WSL2 Distros                                     [ 🔄 Refresh ]

  ☑ Ubuntu (default)
      /home/xanzh · claude ✓ codex ✓ gemini ✗

  ☐ Debian
      /home/user · claude ✗ codex ✗ gemini ✗  [Not installed]

  ☐ Ubuntu-22.04
      /home/dev · claude ✓ codex ✓ gemini ✓
```

- 首次安装/首次进入 Settings：自动 probe；默认 distro 默认勾选，其余不勾
- CLI 全部 ✗ 的 distro 仍可勾选（但 apply 时三个 backend 都 skip，等价于不写）

#### 平台门控
- 整个 WSL 模块用 `#[cfg(target_os = "windows")]` 包裹
- mac/Linux 上 Settings 页不渲染该板块（前端用 Tauri `platform()` 判断）
- 数据库表所有平台都建（无害），但只有 Windows 写入

### 2.3 文件读写层：`wsl.exe` 子进程 backend

新增模块 `crates/llm-relay-core/src/wsl/`：

```
wsl/
  mod.rs        - public API
  distro.rs     - 发现、probe、缓存
  fs.rs         - wsl.exe 子进程读/写/删
  network.rs    - WSL 网关 IP 发现
```

#### `fs.rs` 提供的 API

```rust
pub fn wsl_read(distro: &str, path: &str) -> Result<Option<String>, AppError>;
pub fn wsl_atomic_write(distro: &str, path: &str, bytes: &[u8]) -> Result<(), AppError>;
pub fn wsl_remove(distro: &str, path: &str) -> Result<(), AppError>;
pub fn wsl_exists(distro: &str, path: &str) -> Result<bool, AppError>;
```

#### `wsl_atomic_write` 实现

```rust
// distro 内部用 mktemp + mv -f 做 atomic rename
// 避免 Windows / WSL 互操作的 race；stdin 喂内容避开 shell 转义
let script = r#"
  umask 077
  d="$(dirname "$1")"
  mkdir -p "$d"
  t="$(mktemp "$d/.llmrelay.tmp.XXXXXX")"
  cat > "$t"
  mv -f "$t" "$1"
"#;

Command::new("wsl.exe")
  .args(["-d", distro, "-e", "sh", "-c", script, "_", path])
  .stdin(Stdio::piped())
  // ... 写 bytes 到 stdin
```

#### Backend trait（重构 `config_writer.rs`）

```rust
trait CliBackend {
    fn read(&self, rel_path: &[&str]) -> Result<Option<String>, AppError>;
    fn write_atomic(&self, rel_path: &[&str], bytes: &[u8]) -> Result<(), AppError>;
    fn remove(&self, rel_path: &[&str]) -> Result<(), AppError>;
    fn exists(&self, rel_path: &[&str]) -> Result<bool, AppError>;
    fn label(&self) -> &str;  // "windows" / "wsl-Ubuntu"
}

struct WindowsFsBackend;   // 用 std::fs + dirs::home_dir()
struct WslBackend { distro: String, home: String }  // 用 wsl::fs::*
```

`write_claude_config` / `write_codex_config` / `write_gemini_config` / `clear_*` / 所有 snapshot 函数全部改成接收 `&dyn CliBackend`。Windows 端逻辑零行为变化（现有代码 = 默认走 `WindowsFsBackend`）。

### 2.4 Apply / Clear / Snapshot

#### Snapshot 目录布局

```
~/.llm-relay/cli-config-backup/
  windows.json              ← Windows 本机
  wsl-Ubuntu.json           ← per-distro，按 sanitized 名
  wsl-Ubuntu-22.04.json
```

Sanitize 规则：只保留 `[A-Za-z0-9._-]`，其他换 `_`。

#### `apply_all_configs` 改写

```
targets = [Windows]
        + 每个 selected distro 的 WslBackend

prev_targets = snapshot 目录里现存 *.json 对应的 backend

新加入的 target（snapshot 不存在）→ 先 capture_snapshot，再写入
保留的 target（snapshot 已存在）→ 直接写入（不重新 capture）
被剔除的 target（在 prev 不在 current）→ 用 snapshot restore，删除 snapshot 文件
```

#### `clear_all_configs`（Disable）
- 遍历 snapshot 目录所有 `*.json`
- 每个文件解析出对应 backend（`windows.json` → `WindowsFsBackend`，`wsl-X.json` → `WslBackend{distro="X"}`）
- 对每个 backend 执行 restore
- 删除整个目录

#### 错误隔离
- 某 distro 停了、被 unregister、`wsl.exe` 超时（超过 5s）→ 该 target 记一条 warning 到 events，不影响其它 target
- Apply 整体成功定义：**至少 Windows target 成功**；WSL target 失败仅 warning

### 2.5 老用户迁移

升级到带 WSL2 支持的版本时，启动时一次性：

```rust
if old_path = cli-config-backup.json (file) exists
   && new_dir = cli-config-backup/ does not exist
{
    mkdir new_dir;
    rename old_path -> new_dir/windows.json;
}
```

幂等，安全。

### 2.6 文档

README 新增一节 "在 WSL2 中使用"，说明：
- 默认 distro 自动支持，其它 distro 在 Settings 里勾选
- WSL2 端配置自动写入 `~/.claude/settings.json` 等 Linux 路径
- 用 `host.docker.internal` 而非 IP（不受 WSL 重启影响）
- 重装 distro 前请先在 Settings 里取消勾选或 Disable

---

## 3. 改动文件清单

| 文件 | 改动 |
|---|---|
| `crates/llm-relay-core/src/config_writer.rs` | 拆出 `CliBackend` trait；现有函数全部改为 `&dyn CliBackend` 入参；保留 Windows 路径行为 |
| `crates/llm-relay-core/src/wsl/mod.rs` | 新增（Windows-only） |
| `crates/llm-relay-core/src/wsl/distro.rs` | distro 发现 + probe + 缓存读写 |
| `crates/llm-relay-core/src/wsl/fs.rs` | `wsl_read` / `wsl_atomic_write` / `wsl_remove` / `wsl_exists` |
| `crates/llm-relay-core/src/wsl/network.rs` | WSL 网关 IP 发现 + 变更检测 |
| `crates/llm-relay-core/src/proxy_server.rs` | 接收多个 listener；启动时叠加 WSL 网关 IP listener；周期重 bind |
| `crates/llm-relay-core/src/lifecycle.rs` | 启动时 bind WSL 网关 IP 的 std listener，与 127.0.0.1 listener 一起传给 proxy |
| `crates/llm-relay-core/src/database.rs` | 加 `wsl_distros` 表 migration |
| `crates/llm-relay-core/src/paths.rs` | snapshot 路径从单文件改为目录；老用户迁移函数 |
| `crates/llm-relay-core/src/service.rs` | 启动时跑迁移；apply/clear 走新流程 |
| `src/components/Settings.tsx`（或对应文件） | WSL Distros 板块（仅 Windows 显示） |
| `src-tauri/src/lib.rs` | 暴露 distro 列表 / refresh / toggle 命令 |
| `README.md` | "在 WSL2 中使用" 一节 |

---

## 3.5 WSL2 未安装/未启用时的行为

Windows 可能根本没装 WSL2、装了但没装任何 distro、或装了但被禁用。所有这些场景下 LLM Relay 必须**静默降级**到今天的纯 Windows 行为，不阻塞、不报错、不弹窗。

### 网络层
- `find_wsl_gateway_ip()` 枚举 adapter 找不到 `vEthernet (WSL)` → 返回 `None`
- proxy 只 bind `127.0.0.1:18080`，与现状完全一致
- 周期重检测继续跑；用户事后启用了 WSL → 自动加上 WSL 网关 IP 的 listener，无需重启 Relay

### Distro 发现
`wsl.exe -l -v` 在不同场景下的失败形式：

| 场景 | 表现 |
|---|---|
| 完全没装 WSL（连 `wsl.exe` 都没） | `Command::new("wsl.exe")` 报 `ENOENT` / "program not found" |
| 装了 WSL 但没装 distro | 命令成功，stdout 是 "Windows Subsystem for Linux has no installed distributions" |
| 装了但被禁用（Windows feature 关了） | 返回非零退出码 + 特定错误信息 |
| `wsl.exe` 超时（罕见） | 5s 超时主动 kill |

**所有失败统一视为 "no available distros"**：distro 列表为空，不报错，不区分子类型。

### UI
WSL2 板块在 distro 列表为空时显示：

```
WSL2 Distros
─────────────────────────────────────────────
No WSL2 distros detected.

If you use Claude/Codex/Gemini CLI inside WSL2,
install it via Microsoft Store or `wsl --install`.

[ 🔄 Re-check ]
```

不弹错误对话框、不阻塞 apply、不影响 Windows 端任何功能。

### Apply / Clear
- 没有 selected distro → `apply_all_configs` 只迭代 `[Windows]` target
- snapshot 目录里只有 `windows.json`，行为与今天一致

### 后台检测频率自适应
为避免没装 WSL 的用户被 60s 一次的 `wsl.exe -l -v` 调用扰动：

- 启动时检测一次
- 检测结果为 "WSL 可用且有 distro" → 60s 周期重检测（与健康检查 tick 搭车）
- 检测结果为 "WSL 不可用 / 无 distro" → 切换到**惰性模式**：不再周期检测，仅在用户点 🔄 Re-check 或重启应用时再试

这保证零 WSL 用户的资源开销几乎为 0（启动时一次 ~50ms 的 `wsl.exe -l -v`）。

---

## 4. 不在范围内

- **WSL1**：不支持。`wsl -l -v` 区分版本号，version=1 的 distro 直接过滤掉
- **远程 WSL**（SSH 到另一台机器的 WSL）：完全不在范围
- **写入 distro 内部的 shell rc**（如 `~/.bashrc` 加 `OPENAI_API_KEY=dummy`）：第一版不做，仅写 `~/.codex/auth.json`；如果 Codex CLI 在 WSL 里报缺 env，再加
- **多用户 distro**：probe 用 `whoami`，只支持当前 wsl 启动的默认用户；多用户切换不处理
- **检测 WSL 是否启用 mirror 模式**：不需要，统一走 `host.docker.internal`

---

## 5. 风险与缓解

| 风险 | 缓解 |
|---|---|
| `host.docker.internal` 在 WSL 0.65 以下不存在 | Probe 时 `wsl -d <D> -e getent hosts host.docker.internal`，解析失败给出"请升级 WSL"的提示，并禁用该 distro 的勾选 |
| WSL 网关 IP 变更未及时检测 → WSL 端请求超时 | 60s 重检测；可在 Settings 加 "Reconnect WSL" 按钮立即触发 |
| `wsl.exe -e cat` 启动冷 distro 慢（1-3s） | apply 整体异步；UI 显示 per-target 状态 |
| 用户在 WSL 里手动改了配置后 Disable 会被覆盖 | snapshot 保的是"首次 apply 前"状态。文档说明：手动改过的 WSL 配置请在 Disable 前自行备份 |
| Distro 名含奇怪字符（中文、空格） | snapshot 文件名 sanitize；`wsl -d <D>` 仍传原始名 |
| Tauri 安装包不附带 WSL 二进制依赖 | 完全不依赖，`wsl.exe` 是 Windows 自带 |

---

## 6. 验证计划

实现完成后手动验证：

1. **单 distro 默认场景**：装一个 Ubuntu 当默认 distro，启用 Relay，在 WSL 跑 `claude "hi"` 能通
2. **多 distro 勾选**：勾选 Ubuntu + Debian，apply，检查两个 distro 的 `~/.claude/settings.json` 都被写入
3. **取消勾选**：取消 Debian，apply，确认 Debian 的配置被 restore，Windows + Ubuntu 不受影响
4. **Disable 全局**：关闭 Relay，确认 Windows + 所有勾选 distro 都恢复到首次 apply 前的状态
5. **WSL 重启**：`wsl --shutdown` 然后重启 distro，确认 60s 内 Relay 重新 bind 新网关 IP，WSL 端请求恢复
6. **LAN 隔离**：另一台机器 `curl http://<本机 LAN IP>:18080`，确认 connection refused（物理网卡无 listener）
7. **WSL 卸载**：unregister 一个已勾选 distro，apply/clear 时该 target 报 warning 但流程继续
8. **老用户迁移**：mock 一个 `cli-config-backup.json`，启动后确认自动迁移到 `cli-config-backup/windows.json`
9. **mac/Linux 回归**：在非 Windows 平台跑一遍，确认 Settings 不渲染 WSL 板块、apply/clear 行为零变化
10. **无 WSL 的 Windows**：在没装 WSL 的 Windows 上跑，确认 Settings 显示 "No WSL2 distros detected" + 安装指引，无任何报错，Windows 端 apply/clear 完全正常
11. **WSL 事后启用**：从无 WSL 状态启动 Relay → 安装 WSL + distro → 点 🔄 Re-check 后 distro 出现在列表里，可勾选可 apply
