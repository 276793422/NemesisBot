#!/usr/bin/env node
// 域侧流水线真模型通过率驱动（D4 评测程序面，2026-11-03）。
//
// 四阶段：seed（外部来源建 issue）→ watch（board 导入 + 流转跟踪）→
// classify（终态 + 环节信号 → 建议根因分类）→ report（JSON + MD）。
//
// 零依赖（Node ≥ 22：原生 fetch + WebSocket）。v1 seed 面 = Gitea REST
// （试验靶 gitea/p0admin/durfmt）；Jira 来源手工建单（标题带 marker 即可
// 被 watch 阶段认领——watch/classify 与来源无关）。实跑依赖用户环境
// （模型额度 / Gitea / JIRA 实例 / 三机集群排期）——本脚本交付程序面，
// 实跑与用户协调排期。
//
// 用法（环境变量见 README）：
//   node test-tools/pipeline-eval/run-eval.mjs            # 全四阶段
//   node test-tools/pipeline-eval/run-eval.mjs --watch-only <runid>
//       # 只跟踪既有 run（seed 面外部建单/手工建单后补跟踪）
//
// ⚠️ 成本警告：每个任务 = 一次真实 agent 运行（多轮 LLM + 工具）。
// 建议先 EVAL_TASKS=1 冒烟。驱动建议 ≠ 最终结论——报告带「人工确认」列。

import { writeFileSync, mkdirSync, readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

// ---------------------------------------------------------------------------
// 配置（环境变量；缺省 = 三机试验 master 形态）
// ---------------------------------------------------------------------------

const RUN_ID =
  process.argv.includes("--run-id")
    ? process.argv[process.argv.indexOf("--run-id") + 1]
    : `eval-${new Date().toISOString().replace(/[-:T]/g, "").slice(0, 14)}`;
const WATCH_ONLY = process.argv.includes("--watch-only");

const GATEWAY_WS = process.env.EVAL_GATEWAY_WS ?? "ws://127.0.0.1:46188/ws";
const GATEWAY_TOKEN = process.env.EVAL_GATEWAY_TOKEN ?? "";
const ORIGIN_TYPE = process.env.EVAL_ORIGIN_TYPE ?? "gitea";
const GITEA_BASE = process.env.EVAL_GITEA_BASE ?? "http://127.0.0.1:3000";
const GITEA_TOKEN = process.env.EVAL_GITEA_TOKEN ?? "";
const GITEA_OWNER = process.env.EVAL_GITEA_OWNER ?? "p0admin";
const GITEA_REPO = process.env.EVAL_GITEA_REPO ?? "durfmt";
const TASK_COUNT = Number(process.env.EVAL_TASKS ?? 3);
const TASKS_FILE = process.env.EVAL_TASKS_FILE ?? "";
const IMPORT_TIMEOUT_SECS = Number(process.env.EVAL_IMPORT_TIMEOUT_SECS ?? 300);
const TASK_TIMEOUT_SECS = Number(process.env.EVAL_TASK_TIMEOUT_SECS ?? 7200);
const POLL_SECS = Number(process.env.EVAL_POLL_SECS ?? 15);
const OUT_DIR = process.env.EVAL_OUT_DIR ?? join(process.cwd(), ".pipeline-eval");

if (!GATEWAY_TOKEN) {
  console.error("缺 EVAL_GATEWAY_TOKEN（gateway WS 鉴权 token）");
  process.exit(1);
}
if (!WATCH_ONLY && !GITEA_TOKEN) {
  console.error("缺 EVAL_GITEA_TOKEN（Gitea 建单 PAT）；手工建单用 --watch-only");
  process.exit(1);
}

const MARKER = (n) => `[EVAL-${RUN_ID}-${String(n).padStart(2, "0")}]`;
const now = () => new Date().toISOString();
const sleep = (s) => new Promise((r) => setTimeout(r, s * 1000));

/** 默认任务载荷：小而可验证（跑得完、锚点客观）。换真任务用 EVAL_TASKS_FILE。 */
function defaultTasks() {
  return Array.from({ length: TASK_COUNT }, (_, i) => ({
    title: `${MARKER(i + 1)} 评测任务 ${i + 1}：字符串工具函数补测`,
    body: [
      "## 评测任务（流水线通过率基准）",
      "",
      "在仓库中新增 `eval_utils.py`（或对应语言同等物），实现并测试以下函数：",
      "",
      "1. `slugify(s)`：任意字符串 → kebab-case slug（非字母数字归 `-`，去首尾 `-`，小写）。",
      "2. `truncate(s, n)`：超 n 截断并追加 `...`；不超 n 原样返回。",
      "",
      "[CHECK] 仓库中存在 eval_utils.*（或同等实现文件）且含 slugify 与 truncate 实现",
      "[CHECK] 附带的测试文件存在且能跑过（或同等验证证据在交付评论中）",
    ].join("\n"),
  }));
}

function loadTasks() {
  if (!TASKS_FILE) return defaultTasks();
  if (!existsSync(TASKS_FILE)) {
    console.error(`EVAL_TASKS_FILE 不存在: ${TASKS_FILE}`);
    process.exit(1);
  }
  const parsed = JSON.parse(readFileSync(TASKS_FILE, "utf8"));
  if (!Array.isArray(parsed) || parsed.length === 0) {
    console.error("EVAL_TASKS_FILE 必须是非空数组 [{title, body}]");
    process.exit(1);
  }
  return parsed.map((t, i) => ({
    title: `${MARKER(i + 1)} ${t.title}`,
    body: t.body,
  }));
}

// ---------------------------------------------------------------------------
// 阶段 1：seed（Gitea REST 建单）
// ---------------------------------------------------------------------------

async function gitea(path, method = "GET", body) {
  const resp = await fetch(`${GITEA_BASE}/api/v1${path}`, {
    method,
    headers: {
      "Content-Type": "application/json",
      Authorization: `token ${GITEA_TOKEN}`,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await resp.text();
  let json = null;
  try {
    json = JSON.parse(text);
  } catch {
    /* 非 JSON 保留文本 */
  }
  if (!resp.ok) {
    throw new Error(`Gitea ${method} ${path} → HTTP ${resp.status}: ${text.slice(0, 300)}`);
  }
  return json;
}

async function seed(tasks) {
  console.log(`[seed] 向 ${GITEA_OWNER}/${GITEA_REPO} 建 ${tasks.length} 个 issue…`);
  const seeded = [];
  for (let i = 0; i < tasks.length; i++) {
    const t = tasks[i];
    const created = await gitea(
      `/repos/${GITEA_OWNER}/${GITEA_REPO}/issues`,
      "POST",
      { title: t.title, body: t.body },
    );
    seeded.push({
      seq: i + 1,
      marker: MARKER(i + 1),
      title: t.title,
      gitea_number: created.number,
      origin_id: `${ORIGIN_TYPE}/${GITEA_OWNER}/${GITEA_REPO}#${created.number}`,
      seeded_at: now(),
    });
    console.log(`[seed] ${MARKER(i + 1)} → gitea#${created.number}`);
  }
  return seeded;
}

// ---------------------------------------------------------------------------
// 阶段 2：watch（gateway WS 跟踪 board 单）
// ---------------------------------------------------------------------------

/** 极简 WSAPI 客户端：请求-响应 reqId 配对 + 断线自动重连。 */
function makeWsapi() {
  let ws = null;
  let nextId = 1;
  const pending = new Map(); // reqId -> {resolve, reject, timer}

  function connect() {
    return new Promise((resolve, reject) => {
      const url = `${GATEWAY_WS}?token=${encodeURIComponent(GATEWAY_TOKEN)}`;
      const sock = new WebSocket(url);
      sock.onopen = () => {
        console.log(`[ws] 已连接 ${GATEWAY_WS}`);
        ws = sock;
        resolve();
      };
      sock.onerror = (e) => {
        if (!ws) reject(new Error(`WS 连接失败: ${e.message ?? "unknown"}`));
      };
      sock.onclose = () => {
        if (ws === sock) {
          console.warn("[ws] 连接断开，5s 后重连（pending 请求保留等待）");
          ws = null;
          setTimeout(() => connect().catch(() => {}), 5000);
        }
      };
      sock.onmessage = (ev) => {
        let frame;
        try {
          frame = JSON.parse(typeof ev.data === "string" ? ev.data : "");
        } catch {
          return;
        }
        const rid = frame?.reqId;
        if (rid && pending.has(rid)) {
          const p = pending.get(rid);
          clearTimeout(p.timer);
          pending.delete(rid);
          if (frame.type === "response") {
            if (frame.ok === false || frame.error) p.reject(new Error(frame.error ?? "WSAPI 错误"));
            else p.resolve(frame.data ?? frame.body ?? frame);
          } else {
            p.resolve(frame);
          }
        }
      };
    });
  }

  async function call(module, cmd, data = {}, timeoutSecs = 30) {
    if (!ws || ws.readyState !== WebSocket.OPEN) await connect();
    const reqId = `eval-${nextId++}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(reqId);
        reject(new Error(`WSAPI ${module}.${cmd} 超时（${timeoutSecs}s）`));
      }, timeoutSecs * 1000);
      pending.set(reqId, { resolve, reject, timer });
      ws.send(JSON.stringify({ type: "request", module, cmd, reqId, data }));
    });
  }

  return { connect, call };
}

const TERMINAL = new Set(["done", "cancelled"]);

/** 采集单任务环节信号（导入/流转时间戳 + 评论标记）。 */
async function snapshotTask(wsapi, task) {
  const snap = { ...task };
  // 全量拉 origin_type 匹配的清单，客户端按标题 marker 认领（query 语义
  // 与标题子串匹配无强契约，不依赖）。
  const listed = await wsapi.call("board", "issue.list", {
    origin_type: ORIGIN_TYPE,
    include_cancelled: true,
  });
  const rows = listed?.issues ?? [];
  const hit = rows.find((r) => typeof r.title === "string" && r.title.includes(task.marker));
  if (!hit) return { ...snap, imported: false };

  snap.imported = true;
  snap.board_number = hit.number;
  snap.board_id = hit.id;
  snap.board_title = hit.title;
  snap.status = hit.status;
  snap.stage = hit.stage ?? null;
  const detail = await wsapi.call("board", "issue.get", { number: hit.number });
  const issue = detail?.issue ?? {};
  snap.origin_id_board = issue.origin_id ?? null;
  snap.status_history_first_seen = issue.status;
  const comments = issue.comments ?? [];
  snap.comment_count = comments.length;
  snap.signals = extractSignals(comments.map((c) => String(c.body ?? "")));
  snap.last_activity_at =
    comments.length > 0 ? comments[comments.length - 1]?.created_at ?? null : null;
  return snap;
}

/** 评论标记 → 环节信号（分类器的原始证据面）。 */
function extractSignals(bodies) {
  const all = bodies.join("\n");
  const has = (re) => re.test(all);
  return {
    dispatch_send_failure: has(/⛔/),
    circuit_break_parked: has(/🛑/),
    unsure_human: has(/UNSURE/i),
    redispatch_loop: has(/REDISPATCH_LOOP/i),
    estop: has(/急停|estop/i),
    rework_sub: has(/返工：/),
    anchor_fail: has(/锚点|PLAN_ANCHOR/i),
    review_fail: has(/验收.*FAIL|FAIL.*验收|评审.*不通过/),
  };
}

function suggestClassification(snap) {
  if (!snap.imported) {
    return {
      cls: "scaffold",
      why: "未导入：fetch/connector/flow 引擎机械环节断（查凭据/地址/插件加载）",
    };
  }
  if (snap.status === "done") return { cls: "pass", why: "看板单 done（锚点+评审闭环通过）" };
  if (snap.signals.estop) {
    return { cls: "external", why: "estop/人工干预终态——不算流水线失败" };
  }
  if (snap.signals.dispatch_send_failure || snap.signals.circuit_break_parked) {
    return {
      cls: "scaffold",
      why: "送达失败（⛔）/熔断停车（🛑）——派发链机械环节断",
    };
  }
  if (snap.signals.unsure_human) {
    return {
      cls: "model",
      why: "UNSURE 转人工：链路走通但交付证据不足判定——模型能力（证据面）",
    };
  }
  if (snap.signals.review_fail || (snap.signals.anchor_fail && snap.signals.redispatch_loop)) {
    return {
      cls: "model",
      why: "验收 FAIL / 锚点不过且重派耗尽——链路走通、交付质量不达标",
    };
  }
  if (snap.signals.redispatch_loop) {
    return {
      cls: "scaffold",
      why: "REDISPATCH_LOOP：轮次绑定判环——先查模板/闸门配置再怀疑模型",
    };
  }
  return {
    cls: "unknown",
    why: `终态未达（status=${snap.status}）且无强信号——人工看时间线`,
  };
}

async function watch(wsapi, tasks) {
  const deadlineBySeq = new Map();
  const results = [];
  const startAt = Date.now();
  const importDeadline = startAt + IMPORT_TIMEOUT_SECS * 1000;
  tasks.forEach((t, i) =>
    deadlineBySeq.set(i + 1, importDeadline + TASK_TIMEOUT_SECS * 1000),
  );
  const pending = new Map(tasks.map((t, i) => [i + 1, t]));

  while (pending.size > 0) {
    const nowMs = Date.now();
    for (const [seq, task] of [...pending]) {
      let snap;
      try {
        snap = await snapshotTask(wsapi, task);
      } catch (e) {
        console.warn(`[watch] ${task.marker} 快照失败（下轮重试）: ${e.message}`);
        continue;
      }
      const prev = results.find((r) => r.seq === seq);
      const terminal = snap.imported && TERMINAL.has(snap.status);
      const timedOut = nowMs > deadlineBySeq.get(seq);
      if (terminal || timedOut) {
        snap.timed_out = timedOut && !terminal;
        snap.classification = suggestClassification(snap);
        results.push(snap);
        pending.delete(seq);
        console.log(
          `[watch] ${task.marker} → ${terminal ? snap.status : "超时"}（${snap.classification.cls}）`,
        );
      } else if (!prev) {
        console.log(
          `[watch] ${task.marker} ${snap.imported ? `已导入 board#${snap.board_number}（${snap.status}）` : "等待导入…"}`,
        );
      }
    }
    if (pending.size > 0) await sleep(POLL_SECS);
  }
  return results;
}

// ---------------------------------------------------------------------------
// 阶段 3+4：report（JSON + MD，MD 结构对齐 report-template.md）
// ---------------------------------------------------------------------------

const CLS_LABEL = {
  pass: "通过",
  scaffold: "脚手架",
  payload: "载荷/模板",
  model: "模型能力",
  external: "外部干预",
  unknown: "待人工",
};

function renderMd(run, results) {
  const pass = results.filter((r) => r.classification.cls === "pass").length;
  const lines = [];
  lines.push(`# 域侧流水线通过率评测报告（${RUN_ID}）`);
  lines.push("");
  lines.push(`- 跑批时间：${run.started_at} ~ ${now()}`);
  lines.push(`- 来源：${ORIGIN_TYPE} ${GITEA_OWNER}/${GITEA_REPO}（seed 面）`);
  lines.push(`- 任务数：${results.length}；通过：${pass}（通过率 ${((pass / results.length) * 100).toFixed(0)}%）`);
  lines.push("");
  lines.push("## 结果总表");
  lines.push("");
  lines.push("| # | marker | 外部单 | 看板单 | 终态 | 建议分类 | 依据 | 人工确认 |");
  lines.push("|---|--------|--------|--------|------|----------|------|----------|");
  for (const r of results) {
    lines.push(
      `| ${r.seq} | ${r.marker} | ${r.gitea_number ?? "—"} | ${r.board_number ?? "—"} | ${r.status ?? "未导入"} | ${CLS_LABEL[r.classification.cls] ?? r.classification.cls} | ${r.classification.why} | |`,
    );
  }
  lines.push("");
  lines.push("## 逐任务详情");
  for (const r of results) {
    lines.push("");
    lines.push(`### ${r.marker}（${r.title}）`);
    lines.push("");
    lines.push(`- 外部 issue：${r.gitea_number ?? "—"}；board 单：${r.board_number ?? "—"}（status=${r.status ?? "—"}，stage=${r.stage ?? "—"}）`);
    lines.push(`- 信号：${JSON.stringify(r.signals ?? {})}`);
    lines.push(`- 评论数：${r.comment_count ?? 0}；最后活动：${r.last_activity_at ?? "—"}`);
    if (r.timed_out) lines.push(`- ⚠️ 跟踪超时（导入 ${IMPORT_TIMEOUT_SECS}s / 任务 ${TASK_TIMEOUT_SECS}s 预算内未达终态）`);
    lines.push(`- 现场回查：board 详情 issue#${r.board_number ?? "?"} 时间线 + 评论；集群日志 workspace/logs/cluster_logs/`);
  }
  lines.push("");
  lines.push("## 分类口径（判据）");
  lines.push("");
  lines.push("见 test-tools/pipeline-eval/README.md「根因分类口径」节；驱动建议 ≠ 最终结论，人工确认列必填。");
  lines.push("");
  return lines.join("\n");
}

async function main() {
  mkdirSync(OUT_DIR, { recursive: true });
  const wsapi = makeWsapi();
  await wsapi.connect();
  await wsapi.call("system", "ping", {}, 10).catch(() => null); // 活性探测（失败不致命）

  const tasks = loadTasks();
  const run = { run_id: RUN_ID, started_at: now(), origin_type: ORIGIN_TYPE };
  let seeded = [];
  if (!WATCH_ONLY) {
    seeded = await seed(tasks);
  } else {
    console.log(`[watch-only] 跳过 seed；认领 marker 前缀 [EVAL-${RUN_ID}-]`);
    seeded = tasks.map((t, i) => ({
      seq: i + 1,
      marker: MARKER(i + 1),
      title: t.title,
      gitea_number: null,
      origin_id: null,
      seeded_at: null,
    }));
  }

  const results = await watch(wsapi, seeded);
  const jsonPath = join(OUT_DIR, `report-${RUN_ID}.json`);
  const mdPath = join(OUT_DIR, `report-${RUN_ID}.md`);
  writeFileSync(jsonPath, JSON.stringify({ run, results }, null, 2));
  writeFileSync(mdPath, renderMd(run, results));
  console.log(`[report] ${jsonPath}`);
  console.log(`[report] ${mdPath}`);
  const pass = results.filter((r) => r.classification.cls === "pass").length;
  console.log(`[done] 通过 ${pass}/${results.length}`);
  process.exit(0);
}

main().catch((e) => {
  console.error(`评测驱动失败: ${e.message}`);
  process.exit(1);
});
