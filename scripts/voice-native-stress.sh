#!/usr/bin/env bash
# voice-native-stress.sh — native 并发压测驱动（§四.2，2026-10-16）
#
# 对保留共享能力的三个语音引擎包装（SttEngine / PunctEngine / TtsEngine，
# 审计表 docs/INFO/2026-10-15_voice-ffi-safety-audit.md C-1/C-3/C-8）在真实
# 模型下压并发 &self 调用面。用例本体：crates/nemesis-voice/tests/native_stress.rs
#
# 用法：
#   bash scripts/voice-native-stress.sh --models /path/to/models [--release]
#
# 模型目录布局（同 workspace 的 config/voice.toml models.dir 落地形态）：
#   <models>/stt/sensevoice-small/            (model.onnx + tokens.txt)
#   <models>/punct/ct-transformer-zh-en/      (model.onnx)
#   <models>/tts/kokoro-multi-lang-v1_1/      (kokoro 全套)
#
# 无模型环境 = 诚实 SKIP（退出码 0，打印原因）——不允许静默绿。
set -euo pipefail

MODELS=""
PROFILE="--release"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --models) MODELS="$2"; shift 2 ;;
        --debug)  PROFILE=""; shift ;;
        *) echo "unknown arg: $1"; exit 1 ;;
    esac
done

if [[ -z "$MODELS" ]]; then
    echo "SKIP: no --models given; native stress needs real sherpa-onnx models."
    echo "      Point --models at the dir holding stt/ punct/ tts/ subdirs"
    echo "      (e.g. <workspace>/config/data after 'nemesisbot voice setup')."
    exit 0
fi

# 预检只做提示不做整体拦截（2026-10-16 复查修正）：只装了部分模型的环境
# （如仅 TTS）也应能跑到在场的用例——native_stress.rs 每个用例自带逐模型
# 目录级 SKIP，缺席的用例诚实跳过、在场的照常压测。全部缺席时用例层同样
# 全跳（不静默绿：--nocapture 下打印各用例 SKIP 原因）。
for d in stt/sensevoice-small punct/ct-transformer-zh-en tts/kokoro-multi-lang-v1_1; do
    if [[ ! -e "$MODELS/$d" ]]; then
        echo "note: model dir missing: $MODELS/$d — 对应用例将逐用例 SKIP（'nemesisbot voice setup' 可补齐）。"
    fi
done

export NEMESIS_VOICE_STRESS_MODELS="$MODELS"
echo "== voice native stress (profile: ${PROFILE:-debug}) =="
echo "== models: $MODELS =="
# shellcheck disable=SC2086
cargo test -p nemesis-voice $PROFILE --test native_stress -- --ignored --nocapture --test-threads=1
