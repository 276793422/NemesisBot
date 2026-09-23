// 数据一致性校验：history 应答 vs 磁盘 jsonl 逐字段比对（原始现场 home）。
// 用法：node scripts/verify-history-data-integrity.mjs
//   环境变量：WS_TOKEN WS_PORT HIST_SID JSONL（磁盘对照文件）
// 退出码 0=一致 1=不一致。
import WebSocket from 'file:///C:/AI/NemesisBot_Rust/web/node_modules/ws/index.js';
import fs from 'node:fs';

const TOKEN = process.env.WS_TOKEN || '276793422';
const PORT = process.env.WS_PORT || '49000';
const SID = process.env.HIST_SID || 'f4ef5e14-607c-473e-8c22-5ea6cf65b8a7';
const JSONL = process.env.JSONL
  || 'C:/AI/NemesisBot_Rust/bin/new_windows/.nemesisbot/workspace/logs/session_logs/agent_main_session_f4ef5e14-607c-473e-8c22-5ea6cf65b8a7.jsonl';

// 磁盘真相：逐行解析（role/content），跳过空行。
const disk = fs
  .readFileSync(JSONL, 'utf8')
  .split('\n')
  .filter((l) => l.trim())
  .map((l) => JSON.parse(l));

const ws = new WebSocket(`ws://127.0.0.1:${PORT}/ws?token=${TOKEN}`);
let reqId = 0;
let fail = 0;
const check = (name, ok, detail) => {
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? ' — ' + detail : ''}`);
  if (!ok) fail++;
};

ws.on('open', () => {
  const id = `rq_int_${++reqId}`;
  ws.send(
    JSON.stringify({
      type: 'message',
      module: 'chat',
      cmd: 'history_request',
      data: { request_id: id, limit: 100, session_id: SID }, // 与前端同形态：必须带 session_id
    })
  );
});

let done = false;
ws.on('message', (buf) => {
  if (done) return;
  let j;
  try { j = JSON.parse(buf.toString()); } catch { return; }
  if (!(j.type === 'message' && j.cmd === 'history' && j.data?.request_id === `rq_int_1`)) return;
  done = true;
  const d = j.data;
  const msgs = d.messages || [];

  check('total_count == 磁盘行数', d.total_count === disk.length, `resp=${d.total_count} disk=${disk.length}`);
  check('messages 页长 == 磁盘行数', msgs.length === disk.length, `resp=${msgs.length} disk=${disk.length}`);
  check('session_id 回显', d.session_id === SID, `got=${d.session_id}`);

  // 逐条比对 role + content（顺序必须一致）。
  let mismatch = 0;
  for (let i = 0; i < Math.min(msgs.length, disk.length); i++) {
    const r = msgs[i];
    const g = disk[i];
    const roleEq = (r.role ?? r.message?.role) === g.role;
    const contentEq = (r.content ?? r.message?.content) === g.content;
    if (!roleEq || !contentEq) {
      mismatch++;
      console.log(`      [${i}] role/content 不一致: resp(${r.role}/${JSON.stringify((r.content ?? '').slice(0, 60))}) vs disk(${g.role}/${JSON.stringify((g.content || '').slice(0, 60))})`);
    }
  }
  check('逐条 role+content 一致（顺序同）', mismatch === 0, `不一致 ${mismatch} 条 / ${Math.min(msgs.length, disk.length)} 条`);

  // 字段集完备（前端渲染依赖）：request_id/messages/total_count/has_more/oldest_index/session_id。
  for (const k of ['request_id', 'messages', 'total_count', 'has_more', 'oldest_index', 'session_id']) {
    check(`字段 ${k} 存在`, d[k] !== undefined);
  }
  check('request_id 回显', d.request_id === 'rq_int_1');

  ws.close();
  console.log(fail === 0 ? '\n==== 数据一致性: 全部一致 ====' : `\n==== 数据一致性: ${fail} 项不一致 ====`);
  process.exit(fail === 0 ? 0 : 1);
});

setTimeout(() => { if (!done) { console.log('FAIL  10s 内无 history 应答'); process.exit(1); } }, 10000);
