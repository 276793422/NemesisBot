#!/usr/bin/env bash
# PB-1 纪律守卫：LLM provider「烘焙点」文件级白名单（2026-09-22 方案A 配套）。
#
# 病史：provider 实例在启动装配期被「烘焙」进长生命周期对象，模型热切的
# 运行期联动若靠手写罗列清单必然漏（项目 loop 2026-09-21、集群节点 loop
# 2026-09-22、workflow 引擎、guardian judge、SSE/persona 槽全是同病）。
# 方案A 根治：长生命周期消费者装配时一律用
# nemesis_providers::default_slot::default_following wrapper（跟槽委派），
# 热切写点收敛到 web models.set_default / update_field chokepoint。
# 本守卫防回归：`create_provider` / `create_provider_or_null` 的新调用点
# 只允许出现在下述白名单文件——出现新文件即红，逼作者显式回答「这是自热
# 切路径、一次性 CLI、还是该包 wrapper 的烘焙点」。
#
# 白名单（文件级，新增在此登记并写明理由）：
#   crates/nemesis-providers/src/factory.rs     定义与内部转调
#   nemesisbot/src/agent_factory.rs             主 loop（自热切 set_provider_and_model）+
#                                               cluster loop（wrapper）+ 项目 loop（reload 自热切）
#                                               + 两处 small_model 一次性通道
#   nemesisbot/src/commands/gateway/ctx.rs      workflow/security 快照（wrapper）——
#                                               2026-09-23 gateway 拆解 PB-4/B4 自 gateway.rs
#                                               纯搬迁（随 GatewayCtx 构建），语义零新增
#   nemesisbot/src/commands/gateway/post_agent.rs streaming 槽（wrapper）——
#                                               2026-09-23 gateway 拆解 PB-5/B5 自 gateway.rs
#                                               纯搬迁，语义零新增
#   nemesisbot/src/commands/gateway/runtime.rs  guardian small_model 一次性通道——
#                                               2026-09-23 gateway 拆解 PB-9/B7 自 gateway.rs
#                                               纯搬迁，语义零新增
#   nemesisbot/src/commands/run.rs              一次性 CLI（run --model）
#   nemesisbot/src/commands/model.rs            probe 一次性 CLI
#   nemesisbot/src/projects/manager.rs          ProjectLoopManager::reload_providers 自热切
#   crates/nemesis-providers/src/fallback_provider.rs  fallback 链单一装配单点
#                                                       （机制本身非消费者，语义同
#                                                       factory.rs；调用方在白名单
#                                                       agent_factory，wrapper 语义
#                                                       由调用方决定）
#   nemesisbot/src/commands/session.rs          F5 CLI 分支摘要一次性通道
#                                               （block_on 即跑即退，无长生命周期）
#   nemesisbot/src/commands/gateway/dreaming_job.rs  记忆 dreaming per-sweep 一次性
#                                               通道（cron 每轮重读盘上 config 再
#                                               构造，配置变更自然跟随；同
#                                               runtime.rs guardian 一次性先例）
#   crates/nemesis-web/src/handlers/models.rs   唯一运行期热切写点（chokepoint）
#
# 测试文件（/tests 目录、*_tests.rs / tests.rs）不在纪律范围。
set -uo pipefail
cd "$(dirname "$0")/.."

hits=$(grep -rnE "create_provider(_or_null)?\s*\(" crates nemesisbot/src --include='*.rs' 2>/dev/null \
  | grep -v '/tests' \
  | grep -v '_tests\.rs' \
  | grep -vE '^(crates/nemesis-providers/src/factory\.rs|crates/nemesis-providers/src/fallback_provider\.rs|nemesisbot/src/agent_factory\.rs|nemesisbot/src/commands/gateway/ctx\.rs|nemesisbot/src/commands/gateway/post_agent\.rs|nemesisbot/src/commands/gateway/runtime\.rs|nemesisbot/src/commands/gateway/dreaming_job\.rs|nemesisbot/src/commands/run\.rs|nemesisbot/src/commands/model\.rs|nemesisbot/src/commands/session\.rs|nemesisbot/src/projects/manager\.rs|crates/nemesis-web/src/handlers/models\.rs):' \
  || true)

if [ -n "$hits" ]; then
  echo "ERROR: 白名单外出现新的 LLM provider 装配点（PB-1 烘焙纪律）："
  echo "$hits"
  echo ""
  echo "长生命周期消费者请改用 nemesis_providers::default_slot::default_following"
  echo "包装（模型热切自动跟随）；确属一次性 CLI / 自热切路径需豁免时，"
  echo "在 scripts/check-provider-bake.sh 头部白名单登记理由。"
  exit 1
fi
echo "PB-OK: provider 装配点全部在白名单内（factory/agent_factory/gateway/run/model/manager/models）"
