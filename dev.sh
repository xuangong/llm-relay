#!/bin/bash

# 加载 Rust 环境变量
if [ -f "$HOME/.cargo/env" ]; then
    source "$HOME/.cargo/env"
fi

# 确保 cargo 可用
if ! command -v cargo &> /dev/null; then
    echo "❌ Cargo not found. Please ensure Rust is installed."
    echo "Run: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

echo "✅ Rust environment loaded"
echo "🚀 Starting LLM Relay development server..."
echo ""

# 启动开发服务器
pnpm dev
