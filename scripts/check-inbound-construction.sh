#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# msg_class 构造门禁（docs/PLAN/2026-11-21_消息语义类型化msg_class架构计划.md
# §10 第 3 条【B4】）：全仓禁止绕过构造器家族手写 InboundMessage 字面量——
# 字面量 = msg_class 漏填/误标风险面（serde default=User 陷阱，检测网兜底
# 之外的静态第一道）。
#
# 规则：非白名单文件出现单词边界的 `InboundMessage {` 字面量 = 逃逸（exit 1）。
#   - 构造器家族（user_ingress / notice / instruction / continuation）不在
#     检测面（不带字面量大括号）；
#   - `fn f() -> InboundMessage`（返回类型）、`impl InboundMessage`、
#     `dyn InboundMessage`、`&InboundMessage` 等类型位不匹配 `InboundMessage {`
#     字面量形态（`{` 前必须是词边界空白/行首，类型位是 `>`/`&` 等）。
#
# 白名单（文件级，B4）：
#   - crates/nemesis-types/**        枚举与构造器定义本体（struct 定义处）
#   - crates/nemesis-agent/src/loop/run_loop.rs       hook_prompt 直塞门
#     （2026-11-21 拍板永久不动：enqueue 直进 inbox，不经 busy gate）
#   - crates/nemesis-agent/src/loop/client_spawn.rs   子代理 steer 直塞门
#     （同拍板：enqueue_steer 直进子代理队列）
#   - *tests*.rs / tests.rs / tests/ / *_tests.rs / cov_tests 等   测试文件
#     （合成消息直测闸行为是合法形态；生产面零字面量目标）
#
# 用法：bash scripts/check-inbound-construction.sh   （退出码 0=干净）
# ---------------------------------------------------------------------------
set -u
cd "$(dirname "$0")/.." || exit 1

violations=0
while IFS=$'\t' read -r file lineno text; do
    echo "ESCAPE: $file:$lineno: $text"
    violations=$((violations + 1))
done < <(
    git ls-files '*.rs' | grep -v -E '^crates/nemesis-types/|^crates/nemesis-agent/src/loop/run_loop\.rs$|^crates/nemesis-agent/src/loop/client_spawn\.rs$|(^|/)(tests?\.rs|[^/]*_tests\.rs)$|/tests?/|_tests/' |
        xargs -d '\n' grep -n -E '(^|[^a-zA-Z0-9_])InboundMessage \{' 2>/dev/null |
        grep -v -E '(pub |pub\(crate\) )?struct [A-Za-z]*InboundMessage' |
        grep -v -E '\-> *(nemesis_types::channel::)?InboundMessage \{'
)

if [ "$violations" -gt 0 ]; then
    echo ""
    echo "msg_class 构造门禁：$violations 处字面量逃逸。两条出路："
    echo "  1. 生产代码：改走构造器家族 nemesis_types::channel::{user_ingress, notice, instruction, continuation}"
    echo "     （class 烤死，不依赖手填 msg_class）；"
    echo "  2. 确属拍板直塞门（不经 busy gate 的合法字面量）：把文件加进本脚本白名单并注明拍板出处。"
    exit 1
fi
echo "msg_class 构造门禁：全仓零字面量逃逸（白名单: nemesis-types + 2 直塞门 + 测试文件）"
exit 0
