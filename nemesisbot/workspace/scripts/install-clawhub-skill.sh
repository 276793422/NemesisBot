#!/bin/bash
# install-clawhub-skill.sh
# 从 ClawHub (https://clawhub.ai) 安装 skill 到 NemesisBot

set -e

if [ -z "$2" ]; then
    echo "Usage: $0 <author> <skill-name> [output-name]"
    echo ""
    echo "Example:"
    echo "  $0 steipete weather"
    echo "  $0 steipete weather weather-clawhub"
    exit 1
fi

AUTHOR=$1
SKILL_NAME=$2
OUTPUT_NAME=${3:-"$SKILL_NAME"}

# NemesisBot skills 目录
SKILL_DIR="$HOME/.nemesisbot/workspace/skills/$OUTPUT_NAME"

echo "📦 Installing '$SKILL_NAME' from '$AUTHOR'..."
echo ""

# 创建目录
echo "📁 Creating directory: $SKILL_DIR"
mkdir -p "$SKILL_DIR"

# 下载 SKILL.md
SKILL_URL="https://raw.githubusercontent.com/openclaw/skills/main/skills/$AUTHOR/$SKILL_NAME/SKILL.md"
echo "📥 Downloading from: $SKILL_URL"

curl -f -o "$SKILL_DIR/SKILL.md" "$SKILL_URL"

if [ $? -eq 0 ]; then
    echo ""
    echo "✅ Skill '$OUTPUT_NAME' installed successfully!"
    echo ""
    echo "验证安装:"
    echo "  nemesisbot skills list"
    echo "  nemesisbot skills show $OUTPUT_NAME"
    echo ""
    echo "使用:"
    echo "  nemesisbot agent"
    echo ""
else
    echo ""
    echo "❌ Failed to download skill"
    echo "请检查:"
    echo "  1. 作者名称是否正确"
    echo "  2. Skill 名称是否正确"
    echo "  3. 网络连接是否正常"
    echo ""
    exit 1
fi
