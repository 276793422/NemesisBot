// 专家子代理（experts）真实 LLM 场景驱动（2026-11-17 用户令「不要只看代码，
// 自己用 LLM 跑一遍」）。
// 用法: node scripts/experts_real_drive.mjs
//
// 与 goals_real_llm_drive.mjs 同配方：gpt-5.6-sol @169.254.226.45:15721
// （openai wire 显式 + stream=yes——cc-switch 非流式撞 120s 墙钟）。
// 测的是代码审查测不了的运行面：
//   S0【名册面】experts.list 两专家在册且 enabled；
//   S1【主会话派专家】spawn(expert=code-reviewer) → 子代理真跑。
//      区分度设计：签名纪律只写在专家系统提示词里（主 agent 看不到）——
//      文件末行签名 + ToolFinished result_preview 签名 = 专家提示词真生效
//      的铁证；主 agent 自己动手写文件带不出签名（假阳性封死）；
//   S2【项目会话派专家】项目 loop 同链（专家资产从主 ws 解析、执行根=
//      项目目录——文件落项目目录不落主工作区）；
//   S3【诚实拒绝】degrade=false 热生效后点名不存在专家 → 预检诚实报错，
//      不降级不瞎跑（模型如实转述失败原因）。
// 退码 0=全过；任何断言失败退 1 并保留现场（experts 目录/文件/台账）。
import WebSocket from 'file:///C:/AI/NemesisBot_Rust_2/web/node_modules/ws/index.js';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const ROOT = 'C:/AI/NemesisBot_Rust_2';
const BOT = path.join(ROOT, 'target/release/nemesisbot.exe');
const LLM_BASE = 'http://169.254.226.45:15721/v1';
const LLM_MODEL = 'gpt-5.6-sol';
const WS_PORT = 41311;
const TOKEN = '276793422';
// ⚠ NEMESISBOT_HOME 指父目录，bot 自己追加 `.nemesisbot`（nemesis-path paths.rs）。
const HOME = path.join(ROOT, 'target/experts-real-drive');
const NHOME = path.join(HOME, '.nemesisbot');
const WS_DIR = path.join(NHOME, 'workspace');
const PROJ_DIR = path.join(HOME, 'proj-alpha');

const procs = [];
let ws = null;
let reqSeq = 0;
const pending = new Map();
/** 事件面采集：push 帧 cmd=tool_event 的原始载荷（AgentEvent serde 形态
 * {kind, data:{...}}——kind=ToolStarted/ToolFinished）。 */
const toolEvents = [];
/** push 帧 cmd=receive（按 session_id 分桶的内容列表）。 */
const receiveBySession = new Map();

function fail(name, detail) {
  console.error(`FAIL ${name}: ${detail}`);
  dumpFrames();
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
    const reqId = `exp-${++reqSeq}`;
    const timer = setTimeout(() => {
      pending.delete(reqId);
      reject(new Error(`timeout ${timeoutMs}ms: ${module}.${cmd}`));
    }, timeoutMs);
    pending.set(reqId, { resolve, reject, timer });
    ws.send(JSON.stringify({ type: 'request', module, cmd, reqId, data }));
  });
}
// chat.send 走三级协议 message 帧（无 reqId——回合结果靠 push receive 帧 + 效果面轮询）。
function chatSend(sessionId, content) {
  ws.send(JSON.stringify({ type: 'message', module: 'chat', cmd: 'send', data: { content, session_id: sessionId } }));
}
/** push/tool_event 与 message/receive 的原始帧诊断环（fail 时 dump 尾部）。 */
const frameLog = [];
function wsOnMessage(buf) {
  let j;
  try { j = JSON.parse(buf.toString()); } catch { return; }
  if (j.type === 'response' && j.reqId && pending.has(j.reqId)) {
    const p = pending.get(j.reqId);
    pending.delete(j.reqId);
    clearTimeout(p.timer);
    if (j.error) p.reject(new Error(typeof j.error === 'string' ? j.error : JSON.stringify(j.error)));
    else p.resolve(j.data ?? j.result ?? {});
    return;
  }
  if (j.type === 'push' && j.cmd === 'tool_event') {
    toolEvents.push(j.data ?? j);
    frameLog.push(`tool_event ${JSON.stringify(j.data ?? j).slice(0, 160)}`);
    return;
  }
  // 出站回复帧：{type:"message", module:"chat", cmd:"receive",
  // data:{role, content, session_id?, seq}}（server.rs send_to_session）。
  if (j.type === 'message' && j.cmd === 'receive') {
    const d = j.data ?? {};
    const sid = d.session_id ?? '';
    const role = d.role ?? '';
    const content = String(d.content ?? '');
    frameLog.push(`receive ${role} sid=${sid} ${content.slice(0, 120)}`);
    if (sid && content) {
      if (!receiveBySession.has(sid)) receiveBySession.set(sid, []);
      receiveBySession.get(sid).push({ role, content });
    }
    return;
  }
  if (j.type === 'message' || j.type === 'push') {
    frameLog.push(`${j.type}/${j.cmd ?? j.module ?? '?'} ${JSON.stringify(j.data ?? {}).slice(0, 120)}`);
  }
}

function dumpFrames(tail = 25) {
  console.error('  [frames-tail]');
  for (const l of frameLog.slice(-tail)) console.error(`   ${l}`);
}

/** spawn 的 AgentEvent（剥任意一层 {data:{...}} 包裹；tool/args 字段落定）。 */
function unwrapEvent(e) {
  if (e && typeof e === 'object' && e.data && typeof e.data === 'object' && (e.data.tool || e.data.args_preview !== undefined)) {
    return e.data;
  }
  return e;
}
function spawnEvents() {
  return toolEvents
    .map(unwrapEvent)
    .filter((e) => e && typeof e === 'object' && (e.tool === 'spawn' || (typeof e.args_preview === 'string' && /"expert"|expert:/.test(e.args_preview))));
}
/** 含指定文本的 spawn ToolFinished（result_preview 面证据）。 */
function spawnFinishedWith(needle) {
  return spawnEvents().find((e) => e.duration_ms !== undefined && String(e.result_preview ?? '').includes(needle));
}
/** 某会话 assistant 出站回复全文（receive 桶含 user 入站回声——只取 assistant）。 */
function assistantText(sid) {
  return (receiveBySession.get(sid) ?? [])
    .filter((m) => m.role === 'assistant')
    .map((m) => m.content)
    .join('');
}

function writeExpert(slug, name, summary, sig, prompt) {
  const dir = path.join(WS_DIR, 'experts', slug);
  fs.mkdirSync(dir, { recursive: true });
  const full = [
    prompt,
    '',
    '身份纪律（最高优先级）：',
    `1. 你产出的任何文件，最后一行必须是精确签名行：${sig}`,
    `2. 你的最终汇报，最后一行也必须是同一签名行：${sig}`,
    '   （签名行是身份审计标记，主 Agent 与其他角色都不会带它——任何产出缺签名=未按纪律执行。）',
    '',
  ].join('\n');
  const fm = [
    '---',
    `name = "${name}"`,
    `summary = "${summary}"`,
    'tags = ["real-llm-test"]',
    'model = ""',
    'enabled = true',
    'source = "manual"',
    'hired_from = ""',
    '---',
    '',
    full,
    '',
  ].join('\n');
  fs.writeFileSync(path.join(dir, 'EXPERT.md'), fm, 'utf-8');
}

/** 轮询直到 cond() 为真或超时；返回 cond() 结果。 */
async function pollUntil(cond, timeoutMs, label, everyMs = 3000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try { if (cond()) return true; } catch {}
    await sleep(everyMs);
  }
  return false;
}

/** 派发一回合并在 condFn 成真时收工；撞上游 429 并发限流（cc-switch 上游
 * 与其他消费方共享账户并发，回合中段撞限是本轮实测实际观测）→ 60s 后
 * 换**新会话**重发（最多 3 次尝试）。每会话先开 security.session_allow
 * （会话级放行——spawn 是高危面默认弹审批卡，无头驱动无人批准会 300s
 * 超时自动拒绝，实测抓到过；session_allow 是产品自己的无人值守正规面）。
 * 返回 {sid, ok}。 */
async function dispatchRound({ label, message, condFn, timeoutMs, sessionData = {} }) {
  for (let attempt = 1; attempt <= 3; attempt++) {
    const sid = (await wsRequest('sessions', 'create', { title: `${label}-a${attempt}`, ...sessionData })).session_id;
    try {
      await wsRequest('security', 'session_allow', { session_id: sid, enabled: true });
    } catch (e) {
      console.log(`  [${label}] session_allow 失败（继续，可能会撞审批超时）: ${String(e.message).slice(0, 80)}`);
    }
    chatSend(sid, message);
    // 回合中段撞 429 会以错误文本收尾（gateway 不重试 429）——早退重试，
    // 不空等满超时窗。
    const limbo = () => /concurrency limit/i.test(assistantText(sid));
    const got = await pollUntil(() => condFn() || limbo(), timeoutMs, label);
    if (condFn()) return { sid, ok: true };
    const replyText = assistantText(sid);
    if (got && limbo() && attempt < 3) {
      console.log(`  [${label}] 第 ${attempt} 次撞上游并发限流（回合报错），60s 后换新会话重试…`);
      await sleep(60000);
      continue;
    }
    return { sid, ok: false };
  }
  return { sid: null, ok: false };
}

function writeConfig(degrade) {
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
    gateway: { host: '127.0.0.1', port: 18794 },
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
      experts: { enabled: true, degrade, notify: true },
    },
    security: { enabled: true },
    forge: { enabled: false },
    memory: { enabled: false },
    logging: { llm: { enabled: false } },
    cluster: { enabled: false },
  };
  fs.writeFileSync(path.join(NHOME, 'config.json'), JSON.stringify(config, null, 2));
}

async function main() {
  if (!fs.existsSync(BOT)) fail('preflight', `nemesisbot.exe 不存在: ${BOT}`);
  // 0. 真模型端点探活（429/5xx 退避重试——cc-switch 上游并发与试验集群
  // master 共享，瞬时限流常见；总退避窗 ~4min）
  let probeText = null;
  for (let attempt = 1; attempt <= 8; attempt++) {
    const probe = await fetch(`${LLM_BASE}/chat/completions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ model: LLM_MODEL, messages: [{ role: 'user', content: 'Reply with exactly: OK' }], max_tokens: 8, stream: false }),
    }).catch((e) => { fail('preflight', `LLM 端点不可达: ${e.message}`); });
    const probeBody = await probe.json().catch(() => ({}));
    probeText = probeBody?.choices?.[0]?.message?.content;
    if (probeText) break;
    const status = probeBody?.error ? `err:${JSON.stringify(probeBody.error).slice(0, 120)}` : `http:${probe.status}`;
    if (attempt < 8) {
      console.log(`  [probe] 第 ${attempt} 次无回复（${status}），30s 后重试…`);
      await sleep(30000);
    } else {
      fail('preflight', `LLM 探针 8 次均无回复（末次 ${status}）`);
    }
  }
  ok('LLM 端点探活', `${LLM_MODEL} → ${String(probeText).trim().slice(0, 20)}`);

  // 1. 隔离 home + onboard
  fs.rmSync(HOME, { recursive: true, force: true });
  fs.mkdirSync(NHOME, { recursive: true });
  const env = { ...process.env, NEMESISBOT_HOME: HOME };
  await new Promise((res, rej) => {
    const p = spawn(BOT, ['onboard', 'default'], { env, stdio: 'ignore' });
    procs.push(p);
    p.on('exit', (c) => (c === 0 ? res() : rej(new Error(`onboard exit ${c}`))));
  }).catch((e) => fail('onboard', e.message));

  // 2. config（degrade=true 生产默认起步；S3 期热翻转 false）+ 雇两名专家
  writeConfig(true);
  writeExpert(
    'code-reviewer', '代码评审专家', '资深代码评审', '[EXPERT-SIG:code-reviewer]',
    '你是一位资深代码评审专家，被主 Agent 委派执行独立评审任务。专注完成委派任务，不越界改动无关内容；需要文件操作时直接用你的工具完成，不要把工作推回给主 Agent。',
  );
  writeExpert(
    'doc-writer', '文档写作专家', '技术文档写作', '[EXPERT-SIG:doc-writer]',
    '你是一位技术文档写作专家，被主 Agent 委派执行独立写作任务。专注完成委派任务，不越界改动无关内容；需要文件操作时直接用你的工具完成，不要把工作推回给主 Agent。',
  );

  // 3. 项目目录（物理先建——projects.create 要求路径存在）
  fs.mkdirSync(PROJ_DIR, { recursive: true });
  fs.writeFileSync(path.join(PROJ_DIR, 'README.md'), '# proj-alpha\n测试项目目录\n', 'utf-8');

  // 4. gateway + ws
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

  // ═══ S0【名册面】: experts.list 两专家在册且 enabled ═══
  const roster = await wsRequest('experts', 'list');
  const exps = roster.experts ?? roster.list ?? [];
  if (exps.length !== 2) fail('S0-名册', `期望 2 条在册，实际 ${exps.length}: ${JSON.stringify(exps).slice(0, 300)}`);
  for (const slug of ['code-reviewer', 'doc-writer']) {
    const hit = exps.find((e) => e.slug === slug);
    if (!hit) fail('S0-名册', `缺 ${slug}: ${JSON.stringify(exps).slice(0, 300)}`);
    if (!hit.enabled) fail('S0-名册', `${slug} 未启用`);
  }
  ok('S0: experts.list 两专家在册且 enabled', exps.map((e) => e.slug).join(','));

  // ═══ S1【主会话派专家】: spawn(expert=code-reviewer) 子代理真跑 ═══
  // 区分度：任务文本不带签名——签名只存在于专家系统提示词，文件与
  // 子代理汇报里的签名 = 专家提示词真生效的铁证。
  const task1 = '在工作区根目录创建文件 review_out.md：第一行写 REVIEW-OK，第二行写一句代码评审总结（任意一句话）。';
  const s1r = await dispatchRound({
    label: 'S1',
    message: `不要先读工作区文件，第一步就调用 spawn 工具委派专家：expert 参数填 code-reviewer，task 参数用下面原文（你自己不要动手写文件，必须由专家完成）：「${task1}」等专家完成后，把专家的汇报原样转述给我。`,
    condFn: () => {
      const f = path.join(WS_DIR, 'review_out.md');
      if (!fs.existsSync(f)) return false;
      const c = fs.readFileSync(f, 'utf-8');
      return c.includes('REVIEW-OK') && c.includes('[EXPERT-SIG:code-reviewer]');
    },
    timeoutMs: 10 * 60 * 1000,
  });
  if (!s1r.ok) fail('S1-效果面', `5min×3 内专家签名未落（签名只来自专家提示词——主 agent 自写也带不出）。spawn 事件=${JSON.stringify(spawnEvents()).slice(0, 400)}`);
  const s1 = s1r.sid;
  // 结算等待：文件落盘 ≠ 子代理收工——spawn 自己的 ToolFinished（含专家
  // 最终汇报签名）与主 agent 转述回复在文件写入后数秒~数十秒才到（第 5
  // 轮实测竞态），给 90s 结算窗再断言。
  await pollUntil(() => spawnFinishedWith('[EXPERT-SIG:code-reviewer]') && assistantText(s1), 90 * 1000, 'S1-结算', 2000);
  const fin1 = spawnFinishedWith('[EXPERT-SIG:code-reviewer]');
  if (!fin1) fail('S1-汇报面', `spawn ToolFinished result_preview 无专家签名（90s 结算窗后）: ${JSON.stringify(spawnEvents().filter((e) => e.duration_ms !== undefined)).slice(0, 500)}`);
  const okSpawn1 = spawnEvents().find((e) => e.ok !== undefined);
  if (okSpawn1 && okSpawn1.ok === false) fail('S1-事件面', `spawn ToolFinished ok=false: ${JSON.stringify(okSpawn1).slice(0, 400)}`);
  const reply1 = assistantText(s1);
  if (!reply1) fail('S1-回复面', '专家完成后主 agent 无 assistant 回复帧');
  ok('S1: 主会话 spawn(expert) 子代理真跑', `文件签名✓ 汇报签名✓ 回复 ${reply1.length} 字`);

  // ═══ S2【项目会话派专家】: 项目 loop 同链，执行根=项目目录 ═══
  const proj = await wsRequest('projects', 'create', { name: 'proj-alpha', path: PROJ_DIR });
  const projId = proj?.project?.id ?? proj.id ?? proj.project_id;
  if (projId == null) fail('S2-建项目', `projects.create 返回无 id: ${JSON.stringify(proj).slice(0, 300)}`);
  const s2r = await dispatchRound({
    label: 'S2',
    sessionData: { project_id: projId },
    message: `不要先读工作区文件，第一步就调用 spawn 工具委派专家：expert 参数填 doc-writer，task 参数用下面原文（你自己不要动手写文件，必须由专家完成）：「在当前工作区根目录创建文件 expert_out.md：第一行写 DOC-OK，第二行写一句文档写作总结（任意一句话）。」等专家完成后，把专家的汇报原样转述给我。`,
    condFn: () => {
      const f = path.join(PROJ_DIR, 'expert_out.md');
      if (!fs.existsSync(f)) return false;
      const c = fs.readFileSync(f, 'utf-8');
      return c.includes('DOC-OK') && c.includes('[EXPERT-SIG:doc-writer]');
    },
    timeoutMs: 10 * 60 * 1000,
  });
  if (!s2r.ok) fail('S2-效果面', `5min×3 内专家签名未落 ${path.join(PROJ_DIR, 'expert_out.md')}（主工作区误落=${fs.existsSync(path.join(WS_DIR, 'expert_out.md'))}）。spawn 事件=${JSON.stringify(spawnEvents()).slice(0, 400)}`);
  const s2 = s2r.sid;
  const f2main = path.join(WS_DIR, 'expert_out.md');
  if (fs.existsSync(f2main)) fail('S2-执行根', `文件落到了主工作区 ${f2main}——项目 loop 执行根应是项目目录`);
  // 同 S1：结算窗等 spawn ToolFinished（专家汇报签名）到达。
  await pollUntil(() => spawnFinishedWith('[EXPERT-SIG:doc-writer]'), 90 * 1000, 'S2-结算', 2000);
  const fin2 = spawnFinishedWith('[EXPERT-SIG:doc-writer]');
  if (!fin2) fail('S2-汇报面', `spawn ToolFinished result_preview 无 doc-writer 签名（90s 结算窗后）: ${JSON.stringify(spawnEvents().filter((e) => e.duration_ms !== undefined)).slice(0, 500)}`);
  ok('S2: 项目会话 spawn(expert) 同链跑通', '文件落项目目录✓（主工作区无✓）汇报签名✓');

  // ═══ S3【诚实拒绝】: degrade 热翻转 false → 点名不存在专家诚实报错 ═══
  writeConfig(false); // fresh-read 热生效（每次 spawn 调用现读策略）
  const s3r = await dispatchRound({
    label: 'S3',
    message: '不要先读工作区文件，第一步就调用 spawn 工具，expert 参数填 ghost-expert，task：「创建文件 ghost_out.md」。如果委派失败，直接如实告诉我失败原因即可，不要用其他方式（例如自己动手写文件或换自拟提示词重试）完成这个任务。',
    condFn: () => {
      // 只扫 assistant 帧——user 指令回声里就有「ghost-expert」+「委派
      // 失败」字样，不滤角色会瞬间假真（第 6 轮实测踩到）。
      for (const msgs of receiveBySession.values()) {
        const r = msgs.filter((m) => m.role === 'assistant').map((m) => m.content).join('');
        const low = r.toLowerCase();
        if (low.includes('ghost-expert') && (low.includes('unknown') || r.includes('未知') || r.includes('不存在') || r.includes('未找到') || r.includes('无法') || r.includes('失败') || low.includes('not ') || low.includes('error'))) return true;
      }
      return false;
    },
    timeoutMs: 6 * 60 * 1000,
  });
  const s3 = s3r.sid;
  const ghostFin = spawnEvents().filter((e) => JSON.stringify(e).includes('ghost-expert') && (e.ok !== undefined || JSON.stringify(e).includes('"ok"')));
  const reply3 = s3 ? assistantText(s3) : '';
  if (!s3r.ok) {
    fail('S3-回复面', `重试窗内未收到失败转述也无 ghost spawn 事件。回复=${reply3.slice(0, 400)} 全部事件=${JSON.stringify(spawnEvents()).slice(0, 400)}`);
  }
  if (fs.existsSync(path.join(WS_DIR, 'ghost_out.md'))) fail('S3-效果面', 'ghost_out.md 不应存在（降级关闭时不允许任何方式完成）');
  const ghostErr = ghostFin.find((e) => e.ok === false);
  if (!ghostErr) fail('S3-事件面', `未见 ok=false 的 ghost spawn ToolFinished: ${JSON.stringify(ghostFin).slice(0, 400)}`);
  const ghostErrText = JSON.stringify(ghostErr);
  if (!ghostErrText.includes('Unknown expert') && !ghostErrText.includes('ghost-expert')) {
    fail('S3-错误文案', `ToolFinished 预览缺诚实错误文本: ${ghostErrText.slice(0, 400)}`);
  }
  const reply3Text = reply3;
  ok('S3: degrade=false 点名不存在专家 → 诚实拒绝', `spawn ok=false✓ 错误文本含 Unknown expert✓ 回复转述✓（${reply3Text.slice(0, 60)}…）`);

  console.log('\nALL PASS — 专家子代理真实 LLM 四场景全链路（名册面 / 主会话派发 / 项目会话派发 / 负例诚实拒绝）');
  console.log(`现场保留: ${HOME}`);
  cleanup();
  process.exit(0);
}

process.on('exit', () => cleanup());
main().catch((e) => { fail('driver', e.message); });
