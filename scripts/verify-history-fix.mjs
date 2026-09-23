// 项目会话历史修复 WS 级回归（BUG 2026-09-23）。
//
// 三场景（对 docs/PLAN/2026-09-23_入站过滤链框架与项目会话历史修复.md §6.2）：
//   A      项目目录缺失：history_request → 10s 内 history_response（total≥11），
//          且窗口内无「⚠ …当前不可用」帧（修复前唯一帧就是伪 assistant 消息）。
//   B      目录存在 + reject 模式 + turn 进行中：chat.send → +5s history_request →
//          10s 内 history_response（修复前：排进串行队列，15s 窗口零响应）。
//   legacy 无 session_id 的默认会话 history 照常（回归护栏）。
//
// 前置（由编排者准备，脚本只管连接与断言）：
//   - gateway 以 --local 跑在 <home>（web 端口 49000，token 与 SID 用环境变量覆盖）
//   - 场景 B 需 scripts/mock_slow_llm.py 已在监听（turn 占用 45s）
//
// 用法：node scripts/verify-history-fix.mjs --scenario A|B|legacy
//   环境变量：WS_TOKEN（默认 276793422） WS_PORT（默认 49000）
//             HIST_SID（默认 bugrepro 会话 f4ef5e14-…，total≥11 断言依赖该 home）
//   退出码 0=PASS 1=FAIL。B 场景发完请求即断言收摊，不等 45s turn 结束。
import WebSocket from 'file:///C:/AI/NemesisBot_Rust/web/node_modules/ws/index.js';

const args = process.argv.slice(2);
const scenIdx = args.indexOf('--scenario');
const scenario = scenIdx >= 0 ? args[scenIdx + 1] : 'A';
const TOKEN = process.env.WS_TOKEN || '276793422';
const PORT = process.env.WS_PORT || '49000';
const SID = process.env.HIST_SID || 'f4ef5e14-607c-473e-8c22-5ea6cf65b8a7';
const FENCE_MS = 10000; // 与前端历史加载失败围栏同宽

const ws = new WebSocket(`ws://127.0.0.1:${PORT}/ws?token=${TOKEN}`);
let reqId = 0;
const pending = new Map(); // request_id → {resolve, timer}
let fail = null;
const outboundTexts = [];

function request(cmd, data, timeoutMs) {
  const id = `rq_${++reqId}`;
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      resolve(null); // 超时 = null（调用方断言）
    }, timeoutMs);
    pending.set(id, resolve);
    ws.send(JSON.stringify({ type: 'message', module: 'chat', cmd, data: { ...data, request_id: id } }));
    void timeoutMs;
  });
}

function sleep(ms) { return new Promise((r) => setTimeout(r, ms)); }

ws.on('message', (buf) => {
  let j;
  try { j = JSON.parse(buf.toString()); } catch { return; }
  // 线上帧形态（server.rs send_history_to_session）：ProtocolMessage
  // ("message","chat","history",data)——cmd 是 history，不是 history_response。
  if (j.type === 'message' && j.cmd === 'history' && j.data?.request_id && pending.has(j.data.request_id)) {
    const resolve = pending.get(j.data.request_id);
    pending.delete(j.data.request_id);
    clearTimeout(resolve.timer);
    resolve(j.data);
    return;
  }
  // 任何出站文本帧都留档——场景 A 断言「不可用」伪消息绝不出现。
  const text = j.data?.content;
  if (typeof text === 'string' && text) outboundTexts.push(text);
});

ws.on('error', (e) => { console.error(`[WS] error: ${e.message}`); process.exit(1); });

function report(ok, label, detail) {
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${label}${detail ? ' — ' + detail : ''}`);
  if (!ok) fail = fail || label;
}

ws.on('open', async () => {
  console.log(`[WS] connected (scenario ${scenario}, sid=${SID})`);
  if (scenario === 'A') {
    const t0 = Date.now();
    const resp = await request('history_request', { limit: 20, session_id: SID }, FENCE_MS);
    if (!resp) {
      report(false, 'A: history_response within 10s', '无响应（超时）');
    } else {
      const ms = Date.now() - t0;
      report((resp.total_count || 0) >= 11, 'A: total_count>=11', `total=${resp.total_count} ${ms}ms`);
      report(resp.session_id === SID, 'A: session_id 回显', `got=${resp.session_id}`);
      report((resp.messages?.length || 0) >= 11, 'A: messages 渲染页非空', `len=${resp.messages?.length}`);
    }
    report(
      !outboundTexts.some((t) => t.includes('当前不可用')),
      'A: 无「⚠ 项目…不可用」伪消息',
      outboundTexts.length ? `出站帧 ${outboundTexts.length} 条` : '出站 0 条',
    );
  } else if (scenario === 'B') {
    // ① 基线：turn 未开始时 history 正常（该 home 修复前也通过——证明场景成立）。
    const base = await request('history_request', { limit: 20, session_id: SID }, FENCE_MS);
    report(!!base, 'B① 基线: idle history 正常', base ? `total=${base.total_count}` : '无响应');

    // ② 制造 busy：chat.send 占住项目 loop（mock LLM 45s），5s 后切回发 history。
    console.log('[STEP] chat.send（mock LLM 延迟 45s 占住 loop）');
    ws.send(JSON.stringify({ type: 'message', module: 'chat', cmd: 'send', data: { content: '你好', session_id: SID } }));
    await sleep(5000); // turn 已进入 LLM 等待（确定性占用中）

    const t0 = Date.now();
    const resp = await request('history_request', { limit: 20, session_id: SID }, FENCE_MS);
    if (!resp) {
      report(false, 'B② busy: history_response within 10s', '无响应（排进队列直到 turn 结束——即修复前病灶）');
    } else {
      const ms = Date.now() - t0;
      report(resp.session_id === SID && (resp.total_count || 0) >= 11, 'B② busy: 10s 内应答且归属正确', `total=${resp.total_count} ${ms}ms`);
    }
    console.log('[EXIT] 断言完毕收摊（不等 45s turn）');
  } else {
    // legacy：不带 session_id → 默认会话。
    const resp = await request('history_request', { limit: 20 }, FENCE_MS);
    report(!!resp, 'legacy: history_response 到达', resp ? `total=${resp.total_count} sid=${resp.session_id === undefined ? '(省略)' : resp.session_id}` : '无响应');
  }

  const bad = fail;
  ws.close();
  process.exit(bad ? 1 : 0);
});
