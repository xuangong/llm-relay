#!/bin/bash
set -e

echo "🚀 LLM Relay 开发环境安装脚本"
echo "================================"
echo ""

# 检查 Xcode Command Line Tools
if xcode-select -p &> /dev/null; then
    echo "✅ Xcode Command Line Tools 已安装"
else
    echo "⚠️  需要安装 Xcode Command Line Tools"
    echo "正在安装..."
    xcode-select --install
    echo "请在弹出窗口中完成安装，然后重新运行此脚本"
    exit 1
fi

echo ""

# 检查是否已安装 Rust
if command -v rustc &> /dev/null; then
    echo "✅ Rust 已安装: $(rustc --version)"
else
    echo "📦 安装 Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

    # 加载 Rust 环境变量
    source "$HOME/.cargo/env"

    echo "✅ Rust 安装完成: $(rustc --version)"
fi

# 确保 Rust 环境变量已加载
if [ -f "$HOME/.cargo/env" ]; then
    source "$HOME/.cargo/env"
fi

echo ""
echo "📦 检查包管理器..."

# 检查 pnpm
if ! command -v pnpm &> /dev/null; then
    echo "安装 pnpm..."
    npm install -g pnpm
    echo "✅ pnpm 安装完成: $(pnpm --version)"
else
    echo "✅ pnpm 已安装: $(pnpm --version)"
fi

# 可选：安装 bun（更快的包管理器和运行时）
if ! command -v bun &> /dev/null; then
    echo ""
    read -p "是否安装 bun（推荐，速度更快）? [y/N] " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo "安装 bun..."
        curl -fsSL https://bun.sh/install | bash

        # 加载 bun 环境变量
        if [ -f "$HOME/.bun/bin/bun" ]; then
            export PATH="$HOME/.bun/bin:$PATH"
        fi

        echo "✅ bun 安装完成: $(bun --version)"
    fi
else
    echo "✅ bun 已安装: $(bun --version)"
fi

echo ""
echo "📦 安装项目依赖..."
if [ ! -d "node_modules" ]; then
    pnpm install
else
    echo "✅ node_modules 已存在"
fi

echo ""
echo "🔧 验证 Tauri 环境..."
npx tauri info

echo ""
echo "🔧 验证 Tauri 环境..."
npx tauri info

echo ""
echo "✅ 安装完成！"
echo ""
echo "现在可以运行以下命令启动开发服务器："
echo "  pnpm dev"
echo ""
echo "或者构建生产版本："
echo "  pnpm build"
echo ""
echo "⚠️  首次运行可能需要几分钟来编译 Rust 代码"
