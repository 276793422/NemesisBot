// UAT REL-07 辅助：经 WSAPI models.add 走 dashboard 同款运行时写路径。
// 用法: node scripts/uat_p1_ws_models_add.mjs <真实home(.nemesisbot 目录)>
// 环境变量: WS_TOKEN(网关 web auth_token) WS_PORT(web 端口)
// 协议：WSAPI 请求帧 type="request"+reqId，响应 type="response" 回显 reqId
// （protocol.rs response_ok；chat 的 type="message" 是另一条通路，WSAPI 不认）。
// 退码 0=写成功并落盘  1=失败
import WebSocket from 'file:///C:/AI/NemesisBot_Rust/web/node_modules/ws/index.js';
import fs from 'node:fs';

const home = process.argv[2];
const TOKEN = process.env.WS_TOKEN || '276793422';
const PORT = process.env.WS_PORT || '49000';

const cfgBefore = JSON.parse(fs.readFileSync(home + '/config.json', 'utf8'));
const before = (cfgBefore.model_list || []).length;

const ws = new WebSocket(`ws://127.0.0.1:${PORT}/ws?token=${TOKEN}`);
let done = false;

ws.on('open', () => {
  ws.send(JSON.stringify({
    type: 'request',
    module: 'models',
    cmd: 'add',
    reqId: 'uat-rel7',
    data: {
      name: 'test/uat-ws-model',
      model: 'testai/uat-ws-model',
      key: 'uat-ws-key',
      base_url: 'http://127.0.0.1:8080/v1',
    },
  }));
});

ws.on('message', (buf) => {
  if (done) return;
  let j;
  try { j = JSON.parse(buf.toString()); } catch { return; }
  if (j.type !== 'response' || j.reqId !== 'uat-rel7') return;
  done = true;
  if (j.error) {
    console.log(`WS error: ${JSON.stringify(j.error)}`);
    process.exit(1);
  }
  // 响应成功后再验盘：条目数 +1 且新条目存在。
  const cfg = JSON.parse(fs.readFileSync(home + '/config.json', 'utf8'));
  const list = cfg.model_list || [];
  const added = list.length === before + 1
    && list.some((m) => m.model_name === 'test/uat-ws-model');
  if (!added) {
    console.log(`盘上未生效: before=${before} after=${list.length}`);
    process.exit(1);
  }
  console.log(`models.add 落盘生效 (model_list ${before}->${list.length})`);
  ws.close();
  process.exit(0);
});

ws.on('error', (e) => { console.log(`WS error: ${e.message}`); process.exit(1); });
setTimeout(() => { if (!done) { console.log('timeout: 15s 无 models.add 应答'); process.exit(1); } }, 15000);
