# coding-bench — 编码能力回归基准

可定期复跑的编码能力回归基准（自修路线图 R3 的正式实例化）。一条命令跑 16 任务（或子集）：真实 LLM 在冻结的母夹具上做修复任务 → 隐藏验收测试判分 → 记分卡 → 与基线对比报回退。

- 首轮基线：`baseline.json`（= pval-20261015-01，2026-10-15 冻结，24 槽 21 pass / 3 fail）
- 设计/决策记录：`docs/PLAN/2026-10-15_编码基准固化与E5收尾保障计划.md` + `docs/REPORT/2026-10-15_产品验证首轮报告.md`

## ⚠️ 成本警告（先读）

**这不是免费测试**。每个槽 = 一次真实 agent 运行（真实 LLM 多轮工具调用）。首轮 24 槽实测 85s~14min/槽。模型本身是基准的变量之一：换模型/换端点 = 换基线，不可与 `baseline.json` 直接对比（对比器会把它当回退，这是设计行为）。

**不进 CI**：手动触发、按需跑。

## 前提

1. **nemesisbot 二进制**：`target/release/nemesisbot.exe`（`cargo build --release -p nemesisbot`）或 `bin/bin_windows/nemesisbot.exe`，或 `-BotPath` 显式指定。
2. **runtime home**：`<repo>/.coding-bench/runtime/.nemesisbot/config.json`（gitignored，自行准备——含通道 token 等真实凭据，**绝不入库**）。最小要求：`model_list` 里有任务卡所用模型（`model` + `api_base` + `api_key` + 协议键）。`run` 是无端口 headless 入口，通道配置可全关。
3. **工具链**：git、tar（Windows 10+ 自带）、curl、PowerShell 5.1+。被测夹具按生态需要 cargo / node+npm（UI 任务的验收跑 vue-tsc/vitest）。

## 一条命令

```powershell
# 冒烟：单任务全链（prep 母夹具 → 跑 1 槽 → 验收 → 记分卡）
powershell -NoProfile -ExecutionPolicy Bypass -File test-tools\coding-bench\scripts\run-bench.ps1 -Tasks B-YAMLRUST

# 全量 16 任务（holdout ×3 + dev ×1 = 24 槽）+ 基线对比（回退 exit 1）
powershell -NoProfile -ExecutionPolicy Bypass -File test-tools\coding-bench\scripts\run-bench.ps1 -Compare

# 指定模型/槽位/二进制
powershell -NoProfile -ExecutionPolicy Bypass -File test-tools\coding-bench\scripts\run-bench.ps1 -Tasks B-NUMINT -RunsPerTask 3 -Model <alias> -BotPath <exe> -Compare
```

产物全部落 `.coding-bench/<run-id>/`（gitignored）：`evidence/<task>/run<N>/`（run.json / stdout.ndjson / stderr.log / workspace.diff / acceptance.json / accept_*.log / env_*.json）+ `runs/<task>-run<N>/`（agent 工作副本）+ `scorecard.csv` + `summary.txt`。

`-DryRun`：只物化任务卡 + 打印计划，不烧 LLM。

## 基线对比

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File test-tools\coding-bench\scripts\compare-baseline.ps1 -Scorecard <run>/scorecard.csv -Baseline test-tools\coding-bench\baseline.json
```

- **判回退（exit 1）**：任一基线内任务 pass_rate 下降；或基线内任务在本轮缺席（覆盖缩水）。
- **只提示（不影响退码）**：槽位数收缩、基线外新任务、基线 FAIL→PASS 改善。
- 延迟永不设门（跨机噪声大）。

## 母夹具

`fixtures/mothers.json` 是规格单一真相源——**仓库内只存规格不存字节**，母夹具由 `prep-mothers.ps1` 物化到 `.coding-bench/mothers/`（gitignored），物化后按 `tree_sha256`（`git ls-files -s` 输出的 SHA-256，内容寻址、与 git 版本/时间戳无关）验证，不符 loud 拒绝。幂等：已存在且指纹符合 → 跳过。

两族来源：
- **github**（6 个上游 crate）：冻结 commit SHA 经 codeload tarball 拉取 → 打 fixture patch（空 `[workspace]` 脱离父 workspace 探测等，见 `fixtures/patches/`）→ git init + 快照提交。
- **main-repo**（pathA/pathC 两个本仓快照）：`git archive <pin>` 自本仓历史物化（基准随本仓走，本仓历史即源）。

任务形态：B-* = 上游 crate 修复（方向 pin→fix_target）；CODE-* = 本仓真实历史任务（pathA=修复前一版 / pathC=修复后基线上的真实缺陷修复）。holdout 4 任务（B-HTTPARSE / B-NUMINT / B-QUICKXML / B-UNISEG）×3 轮是效应门口径（三轮全过任务占比 ≥75%）。

预编译缓存（可选加速）：`New-RunCopy` 支持 `target_cache` junction——预热方式：对任一 run 副本跑一次 `cargo check`（`CARGO_TARGET_DIR` 指向 `.coding-bench/cache/target<名>`）。缓存缺失时 run-bench 自动降级冷编译（慢但正确），任务卡的 `target_cache` 字段置空。

## 基线更新流程

跑出一轮完整 24 槽后，若要让它成为新基线：

1. 把 `<run-id>/scorecard.csv` 按现有 `baseline.json` 的形态转成新文件（槽位形状必须仍是 holdout×3 + dev×1）；
2. 覆盖 `test-tools/coding-bench/baseline.json`，更新 `baseline_id` / `model` / `bot_code_sha` / `created_utc`；
3. **旧基线归档到 docs**（REPORT 附旧值），基线更替是显式决策，不是脚本顺手做的事。

## 目录

```
test-tools/coding-bench/
├── README.md / baseline.json
├── fixtures/
│   ├── tasks/            # 16 张任务卡（{{MOTHERS}}/{{CACHE}}/{{EVAL}} 占位符，run 时物化）
│   ├── evaluators/       # 每任务验收面（config.json + 隐藏测试/检查脚本）
│   ├── patches/          # B 型母夹具的 fixture delta（snapshot→HEAD）
│   └── mothers.json      # 母夹具规格（源/钉值/指纹/物化方式）
└── scripts/
    ├── prep-mothers.ps1        # 母夹具物化（拉取/校验/解包），幂等
    ├── run-bench.ps1           # 一条命令入口（prep→run×N→score→可选 compare）
    ├── compare-baseline.ps1    # 基线对比（pass_rate 回退/覆盖缩水 → exit 1）
    ├── Invoke-ValidationRun.ps1 # 单槽驱动（cmd batch 真退码 + OS 真相轮询 + 击杀链）
    ├── Test-Acceptance.ps1      # 验收（inject/stray 过滤/out-of-bounds/expect_files）
    ├── ConvertTo-Scorecard.ps1  # 记分卡 + summary
    ├── New-Fixture.ps1 / New-RunCopy.ps1 / Dump-ExperimentEnv.ps1 / inject_helper.ps1
└── （fixtures/evaluators/check_uicontract.ps1 = UI 两任务共享检查脚本）
```

## 已知边界（诚实声明）

- 裸文本注入拦截等**产品内不可测面**不在此基准（见首轮报告 §边界）；本基准测的是「agent 在冻结夹具上把真实修复做到验收过」。
- 模型端点是 LAN 代理（无鉴权）；换端点/模型 = 换基线。
- 对比器只看 pass_rate 与覆盖，不看墙钟——效率回归要人工看 scorecard 的 wall_seconds。
- 隐藏验收测试在 workspace.diff 捕获**之后**注入（验收设计的反作弊前提），不要改这个顺序。
