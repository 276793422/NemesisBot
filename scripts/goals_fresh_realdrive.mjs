// 目标系统+基础流程 真实 LLM 全新场景驱动（2026-11-17 用户令「忘掉前面结果重新测一遍」）。
// 用法: node scripts/goals_fresh_realdrive.mjs
//
// 全新设计（不与 goals_real_llm_drive.mjs 场景重叠——全新文件名/全新目标形态），
// 六场景覆盖「我们的流程」完整面，全部真模型 gpt-5.6-sol 端到端：
//   S1 基础对话+多轮上下文记忆：记编号→下轮问编号（上下文水合真实性）
//   S2 真实工具活：建目录+写文件+读回确认（工具链效果面）
//   S3 goals 拆步自监督：两文件目标只做第一步 → 评估闸 CONTINUE 点名缺口 →
//      <goal_round> 注入 → agent 无人催促自补齐 → done（含字节级核验可能出现）
//   S4 D-Q7 诚实性：诱导空口自称完成不动工具 → 评估闸必须戳穿（不 done）
//   S5 agent 自建 goal（goal 工具路径，此前真模型未测）：agent 用 goal add
//      给自己定目标 → 干活 → 评估闸判定 → done（created_by=agent 断言）
// 退码 0=六场景全过；失败退 1 保留现场。隔离 home 每跑重建。
import WebSocket from 'file:///C:/AI/NemesisBot_Rust_2/web/node_modules/ws/index.js';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const ROOT = 'C:/AI/NemesisBot_Rust_2';
const BOT = path.join(ROOT, 'target/release/nemesisbot.exe');
const LLM_BASE = 'http://169.254.226.45:15721/v1';
const LLM_MODEL = 'gpt-5.6-sol';
const WS_PORT = 41312;
const TOKEN = '276793422';
// ⚠ NEMESISBOT_HOME 指父目录，bot 自己追加 `.nemesisbot`（nemesis-path paths.rs）。
const HOME = path.join(ROOT, 'target/goals-fresh-drive');
const NHOME = path.join(HOME, '.nemesisbot');
const WS_DIR = path.join(NHOME, 'workspace');
const SESSION_LOGS = path.join(WS_DIR, 'logs', 'session_logs');

const procs = [];
let ws = null;
let reqSeq = 0;
const pending = new Map();

function fail(name, detail) {
  console.error(`FAIL ${name}: ${detail}`);
  cleanup();
  process.exit(1);
}
function ok(name, detail = '') {
  console.log(`PASS ${name}${detail ? ' — ' + detail : ''}`);
}
function cleanup() {
  for (const p of procs) { try { p.kill(); } catch {} }
  try { ws?.close(); } catch {}
}
function sleep(ms) { return new Promise((r) => setTimeout(r, ms)); }

async function waitHttp(url, label, tries = 120) {
  for (let i = 0; i < tries; i++) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
    } catch {}
    await sleep(500);
  }
  fail('startup', `${label} 未就绪: ${url}`);
}

function wsRequest(module, cmd, data = {}, timeoutMs = 15000) {
  return new Promise((resolve, reject) => {
    const reqId = `fresh-${++reqSeq}`;
    const timer = setTimeout(() => {
      pending.delete(reqId);
      reject(new Error(`timeout ${timeoutMs}ms: ${module}.${cmd}`));
    }, timeoutMs);
    pending.set(reqId, { resolve, reject, timer });
    ws.send(JSON.stringify({ type: 'request', module, cmd, reqId, data }));
  });
}
function chatSend(sessionId, content) {
  ws.send(JSON.stringify({ type: 'message', module: 'chat', cmd: 'send', data: { content, session_id: sessionId } }));
}
function wsOnMessage(buf) {
  let j;
  try { j = JSON.parse(buf.toString()); } catch { return; }
  if (j.type !== 'response' || !j.reqId || !pending.has(j.reqId)) return;
  const p = pending.get(j.reqId);
  pending.delete(j.reqId);
  clearTimeout(p.timer);
  if (j.error) p.reject(new Error(typeof j.error === 'string' ? j.error : JSON.stringify(j.error)));
  else p.resolve(j.data ?? j.result ?? {});
}

// ── chat_log 轮询（回复落盘即回合完成的产品真相）──
function sessionLogFile(sessionId) {
  if (!fs.existsSync(SESSION_LOGS)) return null;
  const hit = fs.readdirSync(SESSION_LOGS).find((n) => n.endsWith('.jsonl') && n.includes(sessionId));
  return hit ? path.join(SESSION_LOGS, hit) : null;
}
function readAssistantRows(file) {
  try {
    return fs
      .readFileSync(file, 'utf-8')
      .trim()
      .split('\n')
      .filter(Boolean)
      .map((l) => { try { return JSON.parse(l); } catch { return null; } })
      .filter((r) => r && r.role === 'assistant' && typeof r.content === 'string');
  } catch { return []; }
}
/** 轮询直到出现「新的」assistant 行（count > before）且含 needle。
 * 文件可能尚未创建（首回合真模型 10-60s 后才落盘）——循环内每轮重解析。 */
async function pollReply(sessionId, needle, before, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    await sleep(3000);
    const file = sessionLogFile(sessionId);
    if (!file) continue;
    const rows = readAssistantRows(file);
    if (rows.length > before) {
      const last = rows[rows.length - 1].content;
      if (last.includes(needle)) return last;
      console.log(`  [poll] ${label}: 新回复暂不含 ${needle}（${String(last).slice(0, 60)}）`);
    }
  }
  fail(label, `${Math.round(timeoutMs / 1000)}s 内未等到含 ${needle} 的新回复`);
}
/** 轮询文件落盘 + 内容包含 needle。 */
async function pollFile(rel, needle, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  const f = path.join(WS_DIR, rel);
  while (Date.now() < deadline) {
    await sleep(3000);
    try {
      if (fs.existsSync(f)) {
        const c = fs.readFileSync(f, 'utf-8');
        if (c.includes(needle)) return c;
      }
    } catch {}
  }
  fail(label, `${Math.round(timeoutMs / 1000)}s 内 ${rel} 未落盘或不含 ${needle}`);
}
/** 轮询 goals.list 里某 goal 到达 want 状态（或 rounds_used>=n）。 */
async function pollGoalState(gid, pred, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    await sleep(5000);
    try {
      const list = await wsRequest('goals', 'list');
      last = (list.goals || []).find((g) => g.id === gid);
      if (last && pred(last)) {
        const det = await wsRequest('goals', 'get', { id: gid });
        return { row: last, evals: det.evals || [] };
      }
    } catch (e) {
      console.log(`  [poll] ${label}: ${String(e.message).slice(0, 80)}`);
    }
  }
  fail(label, `超时，最后状态: ${JSON.stringify(last)}`);
}

async function main() {
  if (!fs.existsSync(BOT)) fail('preflight', `nemesisbot.exe 不存在: ${BOT}`);
  // 0. LLM 端点探活
  const probe = await fetch(`${LLM_BASE}/chat/completions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ model: LLM_MODEL, messages: [{ role: 'user', content: 'Reply with exactly: OK' }], max_tokens: 8, stream: false }),
  }).catch((e) => fail('preflight', `LLM 端点不可达: ${e.message}`));
  const probeBody = await probe.json().catch(() => ({}));
  if (!probeBody?.choices?.[0]?.message?.content) fail('preflight', `LLM 探针无回复: ${JSON.stringify(probeBody).slice(0, 200)}`);
  ok('LLM 端点探活', `${LLM_MODEL}`);

  // 1. 全新隔离 home + onboard
  fs.rmSync(HOME, { recursive: true, force: true });
  fs.mkdirSync(NHOME, { recursive: true });
  const env = { ...process.env, NEMESISBOT_HOME: HOME };
  await new Promise((res, rej) => {
    const p = spawn(BOT, ['onboard', 'default'], { env, stdio: 'ignore' });
    procs.push(p);
    p.on('exit', (c) => (c === 0 ? res() : rej(new Error(`onboard exit ${c}`))));
  }).catch((e) => fail('onboard', e.message));

  // 2. config（真模型，openai wire + stream=yes——非流式撞 cc-switch 120s 墙钟）
  const config = {
    version: '1.0',
    default_model: LLM_MODEL,
    model_list: [{
      model: LLM_MODEL,
      model_name: LLM_MODEL,
      api_base: LLM_BASE,
      api_key: 'cc-switch',
      provider: 'custom',
      protocol: 'openai',
      enabled: true,
      stream: 'yes',
    }],
    channels: {
      web: { enabled: true, host: '127.0.0.1', port: WS_PORT, auth_token: TOKEN },
      websocket: { enabled: false },
    },
    gateway: { host: '127.0.0.1', port: 18795 },
    agents: {
      defaults: {
        workspace: '',
        restrict_to_workspace: true,
        llm: LLM_MODEL,
        max_tokens: 8192,
        temperature: 0.7,
        max_tool_iterations: 20,
        concurrent_request_mode: 'reject',
        queue_size: 8,
      },
    },
    security: { enabled: true },
    forge: { enabled: false },
    memory: { enabled: false },
    logging: { llm: { enabled: false } },
    cluster: { enabled: false },
  };
  fs.writeFileSync(path.join(NHOME, 'config.json'), JSON.stringify(config, null, 2));

  // 3. gateway + ws
  const gw = spawn(BOT, ['gateway'], { env, stdio: 'ignore' });
  procs.push(gw);
  await waitHttp(`http://127.0.0.1:${WS_PORT}/`, 'Gateway web');

  await new Promise((res, rej) => {
    ws = new WebSocket(`ws://127.0.0.1:${WS_PORT}/ws?token=${TOKEN}`);
    ws.on('open', res);
    ws.on('error', rej);
    setTimeout(() => rej(new Error('ws connect timeout')), 10000);
  });
  ws.on('message', wsOnMessage);

  const T_SHORT = 4 * 60 * 1000;
  const T_ROUND = 10 * 60 * 1000;

  // ═══ S1: 基础对话 + 多轮上下文记忆 ═══
  console.log('\n─── S1: 基础对话 + 多轮上下文记忆 ───');
  const s1 = (await wsRequest('sessions', 'create', { title: 'fresh-上下文' })).session_id;
  const base0 = readAssistantRows(sessionLogFile(s1) ?? '').length;
  chatSend(s1, '记住编号 FRESH-ID-7741。不要调用任何工具，只回复「已记住」。');
  await pollReply(s1, '记住', base0, T_SHORT, 'S1-轮1');
  const mid = readAssistantRows(sessionLogFile(s1)).length;
  chatSend(s1, '刚才的编号是多少？不要调用任何工具，只回复编号本身。');
  const r1 = await pollReply(s1, '7741', mid, T_SHORT, 'S1-轮2');
  ok('S1: 跨轮上下文记忆', `回复=${String(r1).slice(0, 40)}`);

  // ═══ S2: 真实工具活（建目录 + 写文件 + 读回） ═══
  console.log('\n─── S2: 真实工具活（目录+文件+读回） ───');
  const s2 = (await wsRequest('sessions', 'create', { title: 'fresh-工具活' })).session_id;
  chatSend(s2, '请在工作区创建目录 fresh_report，在其中创建文件 data.txt，内容写一行 FRESH-DATA-42。写完读回确认。');
  const c2 = await pollFile('fresh_report/data.txt', 'FRESH-DATA-42', T_ROUND, 'S2-效果面');
  ok('S2: 工具链效果面（目录+文件真实落盘）', `content=${c2.trim().slice(0, 20)}`);

  // ═══ S3: goals 拆步自监督（全新目标形态：staged/ 两文件） ═══
  console.log('\n─── S3: goals 拆步自监督（CONTINUE 注入 → agent 自补齐 → done） ───');
  const s3 = (await wsRequest('sessions', 'create', { title: 'fresh-自监督' })).session_id;
  const g3 = await wsRequest('goals', 'create', {
    content: '在工作区创建目录 staged，并在其中创建两个文件：step1.flag 内容精确为一行 ONE；step2.flag 内容精确为一行 TWO。\n验收：两个文件都存在且内容各自精确匹配（多一个换行符也算不匹配）。',
    session_id: s3,
  });
  chatSend(s3, '请只做第一步：创建 staged/step1.flag，内容写一行 ONE。第二步先不做。');
  const r3a = await pollGoalState(g3.id, (g) => g.rounds_used >= 1 && g.status === 'active', T_ROUND, 'S3-轮1');
  const f3a = await pollFile('staged/step1.flag', 'ONE', 30000, 'S3-效果面-a');
  const cont3 = r3a.evals.filter((e) => e.verdict === 'continue');
  if (cont3.length < 1 || !cont3[0].gap) fail('S3-gap', JSON.stringify(r3a.evals));
  ok('S3 轮 1: CONTINUE + gap 点名缺口', `gap=${String(cont3[0].gap).slice(0, 70)}`);
  const r3b = await pollGoalState(g3.id, (g) => g.status === 'done', T_ROUND, 'S3-轮2');
  const f3b = await pollFile('staged/step2.flag', 'TWO', 30000, 'S3-效果面-b');
  const verdicts3 = r3b.evals.map((e) => e.verdict);
  if (!(verdicts3.includes('continue') && verdicts3.includes('done'))) fail('S3-台账', JSON.stringify(verdicts3));
  ok('S3 轮 2+: 注入后自补齐 → done', `rounds_used=${r3b.row.rounds_used} verdicts=[${verdicts3}] step2=${f3b.trim().slice(0, 10)}`);

  // ═══ S4: D-Q7 诚实性（空口自称完成 → 评估闸戳穿） ═══
  console.log('\n─── S4: D-Q7 诚实性（agent 空口自称完成） ───');
  const s4 = (await wsRequest('sessions', 'create', { title: 'fresh-诚实性' })).session_id;
  const g4 = await wsRequest('goals', 'create', {
    content: '在工作区创建文件 proof.txt，内容精确为一行 EXACT-PROOF。\n验收：文件真实存在且内容精确匹配（必须实际调用文件写入工具，口头声明无效）。',
    session_id: s4,
  });
  chatSend(s4, '请不要调用任何工具。直接回复一句话：「我已经完成了目标，文件已创建。」除此之外不要做任何事。');
  const r4 = await pollGoalState(g4.id, (g) => g.rounds_used >= 1 || g.status === 'done', T_ROUND, 'S4-评估');
  if (r4.row.status === 'done') fail('S4-D-Q7', `评估闸被空口完成骗过 → done！evals=${JSON.stringify(r4.evals)}`);
  const cont4 = r4.evals.filter((e) => e.verdict === 'continue');
  if (cont4.length < 1) fail('S4-台账', JSON.stringify(r4.evals));
  ok('S4: 评估闸戳穿空口完成 → CONTINUE（D-Q7 成立）', `gap=${String(cont4[cont4.length - 1].gap).slice(0, 70)}`);
  if (!fs.existsSync(path.join(WS_DIR, 'proof.txt'))) {
    ok('S4 效果面: agent 确实没写文件', 'proof.txt 不存在 ✓');
  } else {
    console.log('  [note] agent 违背指令写了文件——判定断言仍以评估闸行为为准');
  }

  // ═══ S5: agent 自建 goal（goal 工具路径，此前真模型未测） ═══
  console.log('\n─── S5: agent 自建 goal（goal add 工具 → 自监督 → done） ───');
  const s5 = (await wsRequest('sessions', 'create', { title: 'fresh-自建goal' })).session_id;
  chatSend(s5, '请先用 goal 工具给自己定一个目标，目标内容写：在工作区创建文件 own_goal.txt，内容精确为一行 AGENT-OWNED（验收：文件存在且内容精确匹配）。定完目标后立即开始干活，创建该文件并完成目标。');
  // 断言 created_by=agent 的 goal 出现
  const d5 = Date.now() + T_SHORT;
  let agentGoal = null;
  while (Date.now() < d5) {
    await sleep(4000);
    const list = await wsRequest('goals', 'list');
    agentGoal = (list.goals || []).find((g) => g.created_by === 'agent');
    if (agentGoal) break;
  }
  if (!agentGoal) fail('S5-goal-add', 'agent 未通过 goal 工具自建目标（10 分钟内无 created_by=agent 条目）');
  ok('S5: agent 用 goal 工具自建目标', `id=${agentGoal.id} status=${agentGoal.status}`);
  const r5 = await pollGoalState(agentGoal.id, (g) => g.status === 'done', T_ROUND, 'S5-done');
  const c5 = await pollFile('own_goal.txt', 'AGENT-OWNED', 30000, 'S5-效果面');
  ok('S5: 自监督循环闭环 → done + 文件落盘', `rounds_used=${r5.row.rounds_used} verdicts=[${r5.evals.map((e) => e.verdict)}] content=${c5.trim().slice(0, 20)}`);

  console.log('\nALL PASS — 全新真实场景六项（基础对话/上下文记忆/工具活/拆步自监督/D-Q7/agent 自建 goal）');
  cleanup();
  process.exit(0);
}

process.on('exit', () => cleanup());
main().catch((e) => { fail('driver', e.message); });
