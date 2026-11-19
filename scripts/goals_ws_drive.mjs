// 目标系统管理面 WSAPI 全命令真网关驱动（2026-11-17 UI/底层功能补测批）。
// 用法: node scripts/goals_ws_drive.mjs
// 前提: target/release/nemesisbot.exe（含 goals feature）+
//       test-tools/TestAIServer/testaiserver.exe 已构建；41300 端口空闲。
// 覆盖: create（正常/缺content/缺session/未占用session/D-Q8重复）/
//       list（空+条目投影）/ get（存在+不存在）/ update（改+空拒+拒改done）/
//       disable→enable 状态机往返 / delete / evals 投影 /
//       真实引擎 blocked(round-limit)→goals.enable 解锁流（D-Q11 管理面）+
//       评估闸 DONE 判定流（rounds_used=0）。
// 引擎流用 testai-goals-1.0 流 marker：goal.md 内嵌 GOALS-BENCH-A（评估恒
// DONE）/ GOALS-BENCH-C（评估恒 CONTINUE，max_rounds=1 → blocked）。
// 退码 0=全过；任何断言失败立即退 1 并打印断言名。
import WebSocket from 'file:///C:/AI/NemesisBot_Rust_2/web/node_modules/ws/index.js';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const ROOT = 'C:/AI/NemesisBot_Rust_2';
const BOT = path.join(ROOT, 'target/release/nemesisbot.exe');
const AI_EXE = path.join(ROOT, 'test-tools/TestAIServer/testaiserver.exe');
const AI_PORT = 18080;
const WS_PORT = 41300;
const TOKEN = '276793422';
// ⚠ NEMESISBOT_HOME 指父目录，bot 自己追加 `.nemesisbot`（nemesis-path paths.rs）。
const HOME = path.join(ROOT, 'target/goals-ws-drive');
const NHOME = path.join(HOME, '.nemesisbot');

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

async function waitHttp(url, label, tries = 60) {
  for (let i = 0; i < tries; i++) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
    } catch {}
    await sleep(500);
  }
  fail('startup', `${label} 30s 未就绪: ${url}`);
}

function wsRequest(module, cmd, data = {}, timeoutMs = 10000) {
  return new Promise((resolve, reject) => {
    const reqId = `drv-${++reqSeq}`;
    const timer = setTimeout(() => {
      pending.delete(reqId);
      reject(new Error(`timeout ${timeoutMs}ms: ${module}.${cmd}`));
    }, timeoutMs);
    pending.set(reqId, { resolve, reject, timer });
    ws.send(JSON.stringify({ type: 'request', module, cmd, reqId, data }));
  });
}

// chat.send 走三级协议 message 帧（无 reqId 无 response——回合结果靠轮询）。
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

async function main() {
  if (!fs.existsSync(BOT)) fail('preflight', `nemesisbot.exe 不存在: ${BOT}`);
  if (!fs.existsSync(AI_EXE)) fail('preflight', `testaiserver.exe 不存在: ${AI_EXE}`);
  fs.rmSync(HOME, { recursive: true, force: true });
  fs.mkdirSync(NHOME, { recursive: true });

  // 1. TestAIServer
  const ai = spawn(AI_EXE, ['--port', String(AI_PORT)], { stdio: 'ignore' });
  procs.push(ai);
  await waitHttp(`http://127.0.0.1:${AI_PORT}/v1/models`, 'TestAIServer');

  // 2. 隔离 home：onboard 提取 workspace 模板（不带 --local——它会覆盖 NEMESISBOT_HOME）。
  const env = { ...process.env, NEMESISBOT_HOME: HOME };
  await new Promise((res, rej) => {
    const p = spawn(BOT, ['onboard', 'default'], { env, stdio: 'ignore' });
    procs.push(p);
    p.on('exit', (c) => (c === 0 ? res() : rej(new Error(`onboard exit ${c}`))));
  }).catch((e) => fail('onboard', e.message));
  if (!fs.existsSync(path.join(NHOME, 'workspace'))) fail('onboard', 'workspace 模板未提取');

  // 3. 覆写 config（bench 配置形态裁剪版：单模型双角色 goals-1.0）。
  const config = {
    version: '1.0',
    default_model: 'test/testai-goals-1.0',
    model_list: [{
      model: 'test/testai-goals-1.0',
      model_name: 'testai-goals-1.0',
      api_base: `http://127.0.0.1:${AI_PORT}/v1`,
      api_key: 'test-key',
      provider: 'test',
      enabled: true,
    }],
    channels: {
      web: { enabled: true, host: '127.0.0.1', port: WS_PORT, auth_token: TOKEN },
      websocket: { enabled: false },
    },
    gateway: { host: '127.0.0.1', port: 18791 },
    agents: {
      defaults: {
        workspace: '',
        restrict_to_workspace: true,
        llm: 'test/testai-goals-1.0',
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

  // 4. Gateway
  const gw = spawn(BOT, ['gateway'], { env, stdio: 'ignore' });
  procs.push(gw);
  await waitHttp(`http://127.0.0.1:${WS_PORT}/`, 'Gateway web');

  // 5. WS 连接
  await new Promise((res, rej) => {
    ws = new WebSocket(`ws://127.0.0.1:${WS_PORT}/ws?token=${TOKEN}`);
    ws.on('open', res);
    ws.on('error', rej);
    setTimeout(() => rej(new Error('ws connect timeout')), 10000);
  });
  ws.on('message', wsOnMessage);

  // ── 查询面：空态
  let list = await wsRequest('goals', 'list');
  if ((list.goals || []).length !== 0) fail('list-empty', JSON.stringify(list));
  ok('goals.list 空态');

  // ── 建两个会话（sessions.create 即写 meta sidecar = occupied，零 LLM）
  const s1 = (await wsRequest('sessions', 'create', { title: '驱动-主' })).session_id;
  const s2 = (await wsRequest('sessions', 'create', { title: '驱动-alt' })).session_id;
  if (!s1 || !s2) fail('sessions-create', JSON.stringify({ s1, s2 }));

  // ── 创建面
  const created = await wsRequest('goals', 'create', {
    content: 'GOALS-BENCH-A 把回复压到一行以内\n验收：回复不含换行',
    session_id: s1,
    max_rounds: 256,
  });
  const gid = created.id;
  if (!gid) fail('create', JSON.stringify(created));
  ok('goals.create 正常', `id=${gid}`);

  try { await wsRequest('goals', 'create', { content: '', session_id: s2 }); fail('create-empty-content', '应拒绝'); }
  catch (e) { ok('goals.create 缺 content 拒绝', String(e.message).slice(0, 60)); }

  try { await wsRequest('goals', 'create', { content: 'x', session_id: '' }); fail('create-empty-session', '应拒绝'); }
  catch (e) { ok('goals.create 缺 session 拒绝', String(e.message).slice(0, 60)); }

  try { await wsRequest('goals', 'create', { content: 'x', session_id: 'no-such-session-xyz' }); fail('create-unoccupied', '应拒绝'); }
  catch (e) { ok('goals.create 未占用会话拒绝', String(e.message).slice(0, 60)); }

  try { await wsRequest('goals', 'create', { content: '第二条', session_id: s1 }); fail('create-dup', 'D-Q8 应拒绝'); }
  catch (e) { ok('goals.create 同会话重复拒绝（D-Q8）', String(e.message).slice(0, 60)); }

  // eval_model/max_rounds 变体（绑另一会话；max_rounds=1 为 blocked 流准备）。
  const created2 = await wsRequest('goals', 'create', {
    content: 'GOALS-BENCH-C small 模型评估的目标',
    session_id: s2,
    max_rounds: 1,
    eval_model: 'small',
  });
  const gid2 = created2.id;
  ok('goals.create eval_model=small + max_rounds=1', `id=${gid2}`);

  // ── 查询面：条目投影
  list = await wsRequest('goals', 'list');
  if ((list.goals || []).length !== 2) fail('list-count', JSON.stringify(list));
  const rowG1 = list.goals.find((g) => g.id === gid);
  const rowG2 = list.goals.find((g) => g.id === gid2);
  if (rowG1.status !== 'active' || rowG1.created_by !== 'human' || rowG1.max_rounds !== 256)
    fail('list-projection', JSON.stringify(rowG1));
  if (rowG2.eval_model !== 'small' || rowG2.max_rounds !== 1)
    fail('list-projection-g2', JSON.stringify(rowG2));
  if (!rowG1.session_key.includes(s1)) fail('list-session-key', rowG1.session_key);
  ok('goals.list 条目投影（status/created_by/max_rounds/eval_model/session_key）');

  // ── 详情面
  let det = await wsRequest('goals', 'get', { id: gid });
  if (!String(det.content || '').includes('GOALS-BENCH-A')) fail('get-content', JSON.stringify(det).slice(0, 200));
  if (!Array.isArray(det.evals)) fail('get-evals-field', 'evals 应为数组');
  ok('goals.get 详情（content + evals 字段）');

  try { await wsRequest('goals', 'get', { id: 'NB-NOPE' }); fail('get-missing', '应诚实错误'); }
  catch (e) { ok('goals.get 不存在诚实错误', String(e.message).slice(0, 60)); }

  // ── 编辑面（改后内容保留 GOALS-BENCH-A——评估闸判别依赖 goal.md 内 marker）
  await wsRequest('goals', 'update', { id: gid, content: 'GOALS-BENCH-A 修正后的目标\n验收：B' });
  det = await wsRequest('goals', 'get', { id: gid });
  if (!String(det.content || '').includes('修正后的目标')) fail('update-content', det.content);
  ok('goals.update 内容修改生效（get 回读）');

  try { await wsRequest('goals', 'update', { id: gid, content: '   ' }); fail('update-empty', '应拒绝'); }
  catch (e) { ok('goals.update 空内容拒绝', String(e.message).slice(0, 60)); }

  // ── 状态机面（G1 走 pause/enable 往返；G2 走引擎 blocked 流）
  await wsRequest('goals', 'disable', { id: gid });
  list = await wsRequest('goals', 'list');
  if (list.goals.find((g) => g.id === gid).status !== 'paused') fail('disable', 'status 应为 paused');
  ok('goals.disable → paused');
  await wsRequest('goals', 'enable', { id: gid });
  list = await wsRequest('goals', 'list');
  if (list.goals.find((g) => g.id === gid).status !== 'active') fail('enable', 'status 应为 active');
  ok('goals.enable → active（往返）');

  // ── 真实引擎流 A：G2（GOALS-BENCH-C 恒 CONTINUE × max_rounds=1）→ blocked(round-limit)
  chatSend(s2, 'GOALS-BENCH-C 开始执行目标');
  let blockedRow = null;
  for (let i = 0; i < 40; i++) {
    await sleep(1500);
    list = await wsRequest('goals', 'list');
    blockedRow = list.goals.find((g) => g.id === gid2);
    if (blockedRow?.status === 'blocked') break;
  }
  if (blockedRow?.status !== 'blocked') fail('engine-blocked', JSON.stringify(blockedRow));
  if (blockedRow.blocked_code !== 'round-limit') fail('engine-blocked-code', blockedRow.blocked_code);
  if (!blockedRow.blocked_message) fail('engine-blocked-msg', 'message 应非空');
  if (blockedRow.rounds_used !== 1) fail('engine-blocked-rounds', String(blockedRow.rounds_used));
  ok('引擎 blocked(round-limit)：code+message+rounds_used=1');
  det = await wsRequest('goals', 'get', { id: gid2 });
  const cont = (det.evals || []).find((e) => e.verdict === 'continue');
  if (!cont || !cont.gap) fail('evals-projection', JSON.stringify(det.evals));
  ok('goals.evals 台账投影（continue + gap 非空）');
  // 独立 goals.evals 命令（八命令之一）与 get.evals 字段同源。
  const ev = await wsRequest('goals', 'evals', { id: gid2 });
  if (!Array.isArray(ev.evals) || !ev.evals.some((e) => e.verdict === 'continue' && e.gap))
    fail('evals-cmd', JSON.stringify(ev));
  ok('goals.evals 独立命令');
  // D-Q11：人 enable 解锁 blocked。
  await wsRequest('goals', 'enable', { id: gid2 });
  list = await wsRequest('goals', 'list');
  const unlocked = list.goals.find((g) => g.id === gid2);
  if (unlocked.status !== 'active' || unlocked.blocked_code !== null)
    fail('blocked-unlock', JSON.stringify(unlocked));
  ok('blocked → goals.enable 解锁回 active（D-Q11，code 清空）');

  // ── 真实引擎流 B：G1（GOALS-BENCH-A 评估恒 DONE）→ done + rounds_used=0
  chatSend(s1, 'GOALS-BENCH-A 执行一次就好');
  let doneRow = null;
  for (let i = 0; i < 40; i++) {
    await sleep(1500);
    list = await wsRequest('goals', 'list');
    doneRow = list.goals.find((g) => g.id === gid);
    if (doneRow?.status === 'done') break;
  }
  if (doneRow?.status !== 'done') fail('engine-done', JSON.stringify(doneRow));
  if (doneRow.rounds_used !== 0) fail('engine-done-rounds', String(doneRow.rounds_used));
  ok('引擎 DONE：rounds_used=0（唯一 done 出口=评估闸）');
  try { await wsRequest('goals', 'update', { id: gid, content: 'GOALS-BENCH-A 改 done 的目标' }); fail('update-done', '应拒改 done'); }
  catch (e) { ok('goals.update 拒改 done', String(e.message).slice(0, 60)); }

  // ── 删除面
  await wsRequest('goals', 'delete', { id: gid });
  list = await wsRequest('goals', 'list');
  if (list.goals.some((g) => g.id === gid)) fail('delete', '列表应无 G1');
  try { await wsRequest('goals', 'get', { id: gid }); fail('delete-get', 'get 应诚实错误'); }
  catch (e) { ok('goals.delete → list 无 + get 诚实错误'); }
  // D-Q8 后半：done 的旧条目保留作历史，同会话可再建（id 单调不复用）。
  const recreated = await wsRequest('goals', 'create', {
    content: 'GOALS-BENCH-A done 后新建替换',
    session_id: s1,
  });
  if (!recreated.id || recreated.id === gid) fail('recreate-after-done', JSON.stringify(recreated));
  ok('done 后同会话可新建（D-Q8 历史保留 + id 不复用）', `${gid} → ${recreated.id}`);
  await wsRequest('goals', 'delete', { id: recreated.id });
  await wsRequest('goals', 'delete', { id: gid2 });

  // 未知命令诚实错误
  try { await wsRequest('goals', 'no-such-cmd', {}); fail('unknown-cmd', '应诚实错误'); }
  catch (e) { ok('goals 未知命令诚实错误', String(e.message).slice(0, 60)); }

  console.log('ALL PASS — goals 管理面 WSAPI 全命令驱动');
  cleanup();
  process.exit(0);
}

process.on('exit', () => cleanup());
main().catch((e) => { fail('driver', e.message); });
