# 构建指南

本文档说明如何为 macOS、Windows、Linux 构建 LLM Relay 安装包。

## 目录

- [前置要求](#前置要求)
- [macOS 构建](#macos-构建)
- [Windows 构建](#windows-构建)
- [Linux 构建](#linux-构建)
- [交叉编译](#交叉编译)
- [构建产物](#构建产物)
- [常见问题](#常见问题)

---

## 前置要求

### 所有平台通用

```bash
# 1. Node.js 18+ 和 pnpm
node --version  # v18 或更高
npm install -g pnpm

# 2. Rust 工具链
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env  # 或重启终端
rustc --version

# 3. 安装项目依赖
pnpm install
```

### 平台特定要求

#### macOS
- macOS 12.0 或更高
- Xcode Command Line Tools: `xcode-select --install`

#### Windows
- Windows 10/11
- [Visual Studio 2022](https://visualstudio.microsoft.com/) 或 [Build Tools for Visual Studio](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022)
  - 安装时勾选 "Desktop development with C++"
- [WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/)（Windows 11 已预装）

#### Linux
- 基础开发工具：
  ```bash
  # Debian/Ubuntu
  sudo apt update
  sudo apt install libwebkit2gtk-4.1-dev \
    build-essential \
    curl \
    wget \
    file \
    libxdo-dev \
    libssl-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev

  # Fedora
  sudo dnf install webkit2gtk4.1-devel \
    openssl-devel \
    curl \
    wget \
    file \
    libappindicator-gtk3-devel \
    librsvg2-devel

  # Arch
  sudo pacman -S webkit2gtk-4.1 \
    base-devel \
    curl \
    wget \
    file \
    openssl \
    appmenu-gtk-module \
    gtk3 \
    libappindicator-gtk3 \
    librsvg \
    libvips
  ```

---

## macOS 构建

### 构建 DMG 安装包

```bash
# 方法 1: 使用 pnpm 脚本
pnpm run tauri build

# 方法 2: 直接使用 Tauri CLI
pnpm tauri build
```

### 构建产物位置

```
src-tauri/target/release/bundle/
├── macos/
│   └── LLM Relay.app          # 应用程序包
└── dmg/
    └── LLM Relay_0.2.4_aarch64.dmg    # Apple Silicon (M1/M2/M3)
    └── LLM Relay_0.2.4_x64.dmg        # Intel (如果在 Intel Mac 上构建)
```

### 为不同架构构建

```bash
# 构建 Apple Silicon (aarch64) 版本
rustup target add aarch64-apple-darwin
pnpm tauri build --target aarch64-apple-darwin

# 构建 Intel (x86_64) 版本
rustup target add x86_64-apple-darwin
pnpm tauri build --target x86_64-apple-darwin

# 构建通用二进制（Universal Binary，同时支持两种架构）
pnpm tauri build --target universal-apple-darwin
```

### 代码签名（可选）

如果要分发给其他用户，需要签名：

```bash
# 设置环境变量
export APPLE_CERTIFICATE="Developer ID Application: Your Name (TEAM_ID)"
export APPLE_ID="your-email@example.com"
export APPLE_PASSWORD="app-specific-password"
export APPLE_TEAM_ID="TEAM_ID"

# 构建并签名
pnpm tauri build
```

---

## Windows 构建

### 在 Windows 上构建

```powershell
# 1. 安装 Rust (PowerShell)
winget install Rustlang.Rustup

# 或使用 rustup-init.exe from https://rustup.rs/

# 2. 安装 Node.js 和 pnpm
winget install OpenJS.NodeJS
npm install -g pnpm

# 3. 克隆项目并安装依赖
git clone <your-repo-url>
cd llm-relay
pnpm install

# 4. 构建
pnpm run tauri build
```

### 构建产物位置

```
src-tauri\target\release\bundle\
├── nsis\
│   └── LLM Relay_0.2.4_x64-setup.exe     # NSIS 安装器（推荐）
└── msi\
    └── LLM Relay_0.2.4_x64_en-US.msi     # MSI 安装器
```

### 选择安装器类型

```powershell
# 只构建 NSIS 安装器
pnpm tauri build -- --bundles nsis

# 只构建 MSI 安装器
pnpm tauri build -- --bundles msi

# 构建两种安装器（默认）
pnpm tauri build
```

### 代码签名（可选）

Windows 安装器可以使用代码签名证书：

```powershell
# 方法 1: 使用环境变量
$env:TAURI_SIGNING_PRIVATE_KEY = "path\to\certificate.pfx"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "your-password"
pnpm tauri build

# 方法 2: 在 tauri.conf.json 中配置
# "windows": {
#   "certificateThumbprint": "THUMBPRINT",
#   "timestampUrl": "http://timestamp.digicert.com"
# }
```

---

## Linux 构建

### 在 Linux 上构建

```bash
# 1. 安装系统依赖（见前置要求）

# 2. 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# 3. 安装 Node.js 和 pnpm
curl -fsSL https://deb.nodesource.com/setup_18.x | sudo -E bash -
sudo apt install -y nodejs
npm install -g pnpm

# 4. 克隆项目并构建
git clone <your-repo-url>
cd llm-relay
pnpm install
pnpm run tauri build
```

### 构建产物位置

```
src-tauri/target/release/bundle/
├── deb/
│   └── llm-relay_0.2.4_amd64.deb      # Debian/Ubuntu 包
├── rpm/
│   └── llm-relay-0.2.4-1.x86_64.rpm   # Fedora/RHEL 包
└── appimage/
    └── llm-relay_0.2.4_amd64.AppImage # AppImage（通用）
```

### 选择打包格式

```bash
# 只构建 deb 包
pnpm tauri build -- --bundles deb

# 只构建 AppImage
pnpm tauri build -- --bundles appimage

# 构建所有格式（默认）
pnpm tauri build
```

---

## 交叉编译

### 从 macOS 构建 Windows 版本

⚠️ **不推荐** - 交叉编译 Windows 版本很复杂，建议直接在 Windows 上构建。

如果确实需要：

```bash
# 1. 安装 Windows 目标
rustup target add x86_64-pc-windows-msvc

# 2. 安装 Wine（用于运行 Windows 工具）
brew install --cask wine-stable

# 3. 尝试构建（可能失败）
pnpm tauri build --target x86_64-pc-windows-msvc
```

常见问题：需要 Windows SDK、链接器等，非常复杂。**强烈建议直接在 Windows 上构建。**

### 从 Windows 构建 macOS 版本

❌ **不可行** - Windows 无法构建 macOS 应用，因为需要 Apple 的工具链。

### 从 Linux 构建其他平台

与 macOS 类似，交叉编译 Windows 非常困难。推荐在各自平台上原生构建。

---

## 构建产物

### 各平台安装包对比

| 平台 | 格式 | 大小（估算） | 推荐 | 说明 |
|------|------|------------|------|------|
| macOS | DMG | ~15-20MB | ✅ | 标准 macOS 安装方式，拖拽安装 |
| macOS | .app | ~15MB | - | 应用程序包，可直接运行 |
| Windows | NSIS .exe | ~20-25MB | ✅ | 现代安装向导，支持静默安装 |
| Windows | MSI | ~20-25MB | - | 传统 Windows 安装器 |
| Linux | .deb | ~15-20MB | ✅ | Debian/Ubuntu 包管理器 |
| Linux | .rpm | ~15-20MB | ✅ | Fedora/RHEL 包管理器 |
| Linux | AppImage | ~25-30MB | ✅ | 通用格式，无需安装 |

### 版本号管理

版本号定义在以下文件中，构建前需要保持一致：

```json
// package.json
{
  "version": "0.2.4"
}

// src-tauri/Cargo.toml
[package]
version = "0.2.4"

// src-tauri/tauri.conf.json
{
  "version": "0.2.4"
}
```

更新版本号：

```bash
# 手动编辑上述三个文件，然后
pnpm install  # 更新 package-lock.json 和 Cargo.lock
```

---

## 常见问题

### 1. 构建失败：找不到 `tauri` 命令

```bash
# 全局安装 Tauri CLI
npm install -g @tauri-apps/cli

# 或使用项目本地的 Tauri CLI
pnpm tauri build
```

### 2. Windows: 缺少 Visual Studio 或 Windows SDK

**错误信息**：
```
error: linker `link.exe` not found
```

**解决方案**：
1. 安装 [Visual Studio 2022 Community](https://visualstudio.microsoft.com/)
2. 在安装器中勾选 "Desktop development with C++"
3. 重启终端并重试

### 3. Linux: 缺少 webkit2gtk

**错误信息**：
```
Package webkit2gtk-4.1 was not found
```

**解决方案**：
```bash
# Ubuntu/Debian
sudo apt install libwebkit2gtk-4.1-dev

# Fedora
sudo dnf install webkit2gtk4.1-devel

# Arch
sudo pacman -S webkit2gtk-4.1
```

### 4. macOS: 代码签名失败

**错误信息**：
```
errSecInternalComponent
```

**解决方案**：
1. 确保证书已导入到钥匙串
2. 使用正确的 Team ID 和证书名称
3. 如果不需要签名，可以跳过（仅用于开发）

### 5. 构建速度慢

**优化方案**：

```bash
# 1. 使用增量编译（默认开启）
cargo build --release

# 2. 使用更快的链接器
# 在 ~/.cargo/config.toml 中添加：
[target.x86_64-apple-darwin]
rustflags = ["-C", "link-arg=-fuse-ld=lld"]

# 3. 使用更多 CPU 核心
cargo build --release -j 8
```

### 6. 构建产物体积过大

**优化方案**：

```toml
# 在 src-tauri/Cargo.toml 中添加
[profile.release]
opt-level = "z"     # 优化体积
lto = true          # 链接时优化
codegen-units = 1   # 更好的优化
strip = true        # 移除调试符号
```

### 7. 前端资源 bundle 过大

**警告信息**：
```
Some chunks are larger than 500 kB after minification
```

**当前可以忽略** - 这个警告不影响桌面应用性能。

如果想优化：
```typescript
// vite.config.ts
export default {
  build: {
    rollupOptions: {
      output: {
        manualChunks: {
          'react-vendor': ['react', 'react-dom'],
          'ui-vendor': ['@radix-ui/react-dialog', '@radix-ui/react-label']
        }
      }
    }
  }
}
```

---

## CI/CD 自动化构建

### GitHub Actions 示例

创建 `.github/workflows/build.yml`：

```yaml
name: Build Release

on:
  push:
    tags:
      - 'v*'

jobs:
  build-macos:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v2
        with:
          version: 8
      - uses: actions/setup-node@v4
        with:
          node-version: '18'
          cache: 'pnpm'
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-apple-darwin,x86_64-apple-darwin
      - run: pnpm install
      - run: pnpm tauri build --target universal-apple-darwin
      - uses: actions/upload-artifact@v4
        with:
          name: macos-dmg
          path: src-tauri/target/release/bundle/dmg/*.dmg

  build-windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v2
        with:
          version: 8
      - uses: actions/setup-node@v4
        with:
          node-version: '18'
          cache: 'pnpm'
      - uses: dtolnay/rust-toolchain@stable
      - run: pnpm install
      - run: pnpm tauri build
      - uses: actions/upload-artifact@v4
        with:
          name: windows-installer
          path: src-tauri/target/release/bundle/nsis/*.exe

  build-linux:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v2
        with:
          version: 8
      - uses: actions/setup-node@v4
        with:
          node-version: '18'
          cache: 'pnpm'
      - uses: dtolnay/rust-toolchain@stable
      - name: Install dependencies
        run: |
          sudo apt update
          sudo apt install -y libwebkit2gtk-4.1-dev \
            build-essential curl wget file \
            libssl-dev libayatana-appindicator3-dev librsvg2-dev
      - run: pnpm install
      - run: pnpm tauri build
      - uses: actions/upload-artifact@v4
        with:
          name: linux-packages
          path: |
            src-tauri/target/release/bundle/deb/*.deb
            src-tauri/target/release/bundle/appimage/*.AppImage
```

---

## 快速参考

### 命令速查表

```bash
# 开发模式（热重载）
pnpm tauri dev

# 生产构建（所有格式）
pnpm tauri build

# 指定目标平台
pnpm tauri build --target <target>

# 指定打包格式
pnpm tauri build -- --bundles <format>

# 检查 Rust 代码（不构建）
cargo check

# 清理构建缓存
cargo clean
rm -rf dist src-tauri/target
```

### 构建目标速查

| 平台 | 目标三元组 |
|------|-----------|
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Universal | `universal-apple-darwin` |
| Windows 64-bit | `x86_64-pc-windows-msvc` |
| Linux 64-bit | `x86_64-unknown-linux-gnu` |

### 打包格式速查

| 平台 | 格式名称 |
|------|---------|
| macOS | `dmg`, `app` |
| Windows | `nsis`, `msi` |
| Linux | `deb`, `rpm`, `appimage` |

---

## 进阶配置

### 自定义图标

图标位置：`src-tauri/icons/`

要求：
- macOS: `icon.icns` (至少包含 512x512)
- Windows: `icon.ico` (包含多种尺寸)
- Linux: PNG 格式，多种尺寸

生成图标：
```bash
# 使用 Tauri CLI 自动生成所有格式
pnpm tauri icon path/to/source-image.png
```

### 自定义安装器

**Windows NSIS 自定义脚本**：

编辑 `src-tauri/tauri.conf.json`：
```json
{
  "bundle": {
    "windows": {
      "nsis": {
        "installerIcon": "icons/icon.ico",
        "installMode": "currentUser",
        "languages": ["en-US", "zh-CN"],
        "displayLanguageSelector": true,
        "installerHooks": "./installer-hooks"
      }
    }
  }
}
```

**macOS DMG 背景图**：

在 `src-tauri/` 创建 `dmg-background.png` 或在配置中指定。

---

## 需要帮助？

- [Tauri 官方文档](https://tauri.app/v1/guides/)
- [Tauri Discord 社区](https://discord.gg/tauri)
- [项目 Issues](https://github.com/xuangong/llm-relay/issues)
