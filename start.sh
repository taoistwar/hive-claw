#!/bin/bash
# 离线分析 AI Agent 启动脚本

set -e

echo "=== 离线分析 AI Agent ==="
echo ""

# 检查配置文件
if [ ! -f "config.toml" ]; then
    echo "⚠️  配置文件 config.toml 不存在"
    echo "  正在从示例配置创建..."
    cp config.example.toml config.toml
    echo "  ✅ 已创建 config.toml"
    echo ""
    echo "  ⚠️  请编辑 config.toml 填入实际配置："
    echo "     - Azkaban 地址和凭据"
    echo "     - Hive Metastore (MySQL) 连接"
    echo "     - Git 仓库地址"
    echo "     - AI API Key"
    echo ""
    echo "  按回车继续启动（使用默认配置）..."
    read -r
fi

# 检查编译
if [ ! -f "target/release/offline-analysis-agent" ]; then
    echo "🔨 正在编译 release 版本..."
    cargo build --release
    echo "  ✅ 编译完成"
    echo ""
fi

# 启动应用
echo "🚀 启动应用..."
echo ""
./target/release/offline-analysis-agent
