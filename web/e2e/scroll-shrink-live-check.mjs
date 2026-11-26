/**
 * scroll-shrink-live-check.mjs — ChatPanel 滚动跟随「收缩误杀」真浏览器验证。
 *
 * 背景：2026-11-26 根因 = 2026-11-25 拽底循环修复引入的方向守卫把「几何性
 * scrollTop 下移」（钉底时内容收缩 → stick 重钉 / clamp）误判成用户上滚 →
 * follow 永久解除 → HOOK 处理过程堆积视口下方。
 *
 * 本脚本用真实 chromium（真实 MutationObserver / 滚动事件 / clamp 语义 /
 * overflow-anchor:none，与 ChatPanel .chat-messages 同款样式）对新旧两版
 * useStickToBottom 对跑同一组场景：
 *   - 旧版（git HEAD 提取，修复未提交时 HEAD=旧版）必须复现出 BUG——证明
 *     测试本身有效、场景就是线上症状；
 *   - 新版（工作区源码）必须全过——证明修复在真实浏览器语义下成立；
 *   - 11-25 拽底循环语义（上滚不重臂/不拽底/滚回恢复）在新版下零回退。
 *
 * 用法：node web/e2e/scroll-shrink-live-check.mjs
 * 环境变量：SCROLLCHECK_KEEP=1 保留 .tmp-scrollcheck/ 产物。
 * 退出码：0=全部符合预期；1=任一场景不符（新版回归或旧版未复现）。
 */
import path from 'node:path'
import fs from 'node:fs'
import { execFileSync, spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')
const webRoot = path.join(repoRoot, 'web')
const require = createRequire(path.join(webRoot, 'package.json'))
const { chromium } = require('playwright')

const TMP = path.join(webRoot, 'e2e', '.tmp-scrollcheck')
fs.mkdirSync(TMP, { recursive: true })
const keep = process.env.SCROLLCHECK_KEEP === '1'
const cleanup = () => { if (!keep) fs.rmSync(TMP, { recursive: true, force: true }) }
process.on('exit', cleanup)
process.on('SIGINT', () => { cleanup(); process.exit(130) })

// --- 1. 旧版模块：git HEAD 提取（修复未提交时 HEAD 即修复前代码） ----------
const oldSrc = (() => {
  const r = spawnSync('git', ['show', 'HEAD:web/src/composables/useStickToBottom.ts'], {
    cwd: repoRoot, encoding: 'utf8',
  })
  if (r.status !== 0 || !r.stdout) throw new Error(`git show HEAD 失败：${r.stderr}`)
  if (!r.stdout.includes('lastScrollTop: number')) {
    console.log('[SKIP-ARM] HEAD 已是修复后代码（旧守卫签名不存在）——复现臂跳过，只验证新版。')
    return null
  }
  const p = path.join(TMP, 'stick-old.ts')
  fs.writeFileSync(p, r.stdout)
  return p
})()

// --- 2. esbuild 打包（iife 全局名，浏览器直载）-----------------------------
function bundle(entry, globalName, outfile) {
  let esbuild
  try {
    esbuild = require('esbuild')
  } catch {
    // 兜底：走 npx（慢路径）
    const r2 = spawnSync('npx', ['esbuild', entry, '--bundle', '--format=iife',
      `--global-name=${globalName}`, `--outfile=${outfile}`],
      { cwd: webRoot, encoding: 'utf8', shell: process.platform === 'win32' })
    if (r2.status !== 0) throw new Error(`esbuild(npx) 失败：${r2.stderr}`)
    return
  }
  esbuild.buildSync({
    entryPoints: [path.resolve(webRoot, entry)],
    bundle: true, format: 'iife', globalName,
    outfile: path.join(TMP, outfile),
    logLevel: 'silent',
  })
}

bundle('src/composables/useStickToBottom.ts', 'StickModuleNew', 'stick-new.js')
if (oldSrc) bundle(path.relative(webRoot, oldSrc), 'StickModuleOld', 'stick-old.js')

// --- 3. 页面骨架（镜像 ChatPanel .chat-messages 关键样式） ------------------
const html = `<!doctype html><html><head><meta charset="utf-8"><style>
  .chat-messages { height: 400px; width: 600px; overflow-y: auto; overflow-anchor: none; border: 1px solid #888; }
  /* padding 代替 margin 做间距——margin 纵向折叠会让高度算术失去确定性 */
  .msg { height: 88px; padding: 4px 0; margin: 0 4px; background: #2c3244; border-radius: 6px; }
  .live { background: #3d2f34; }
  .live .detail { height: 40px; padding: 4px 0; margin: 0 4px; background: #57404a; }
</style>
<script src="stick-new.js"></script>
${oldSrc ? '<script src="stick-old.js"></script>' : ''}
</head><body><div id="host"></div></body></html>`
fs.writeFileSync(path.join(TMP, 'index.html'), html)

// --- 4. 页内场景 runner ------------------------------------------------------
const pageSource = `
window.runScenario = async (modName, scenario) => {
  const settle = () => new Promise((r) => requestAnimationFrame(() =>
    requestAnimationFrame(() => setTimeout(r, 30))))
  const dist = (el) => el.scrollHeight - el.scrollTop - el.clientHeight
  const mod = window[modName]
  const host = document.getElementById('host')
  host.innerHTML = ''
  const el = document.createElement('div')
  el.className = 'chat-messages'
  host.appendChild(el)
  const c = mod.useStickToBottom(() => el)
  const addMsg = (cls) => { const d = document.createElement('div'); d.className = 'msg ' + (cls||''); el.appendChild(d); return d }
  const addMsgs = (n, cls) => { let last; for (let i=0;i<n;i++) last = addMsg(cls); return last }
  const addLive = () => { const d = document.createElement('div'); d.className = 'msg live'; d.dataset.live='1'; el.appendChild(d); return d }
  const trace = []
  const snap = (tag) => trace.push({ tag, follow: c.follow.value, dist: dist(el),
    st: el.scrollTop, sh: el.scrollHeight })

  c.attach()
  await settle()

  if (scenario === 'shrink_repin') {
    addMsgs(4); await settle()                 // 384px < 400px：贴底，follow=true
    addMsgs(3); await settle()                 // 672px：MO → stick 钉底
    snap('grown_pinned')
    // 收缩（= 轮末 live 卸载换折叠卡）：3 条一起摘，-288px；MO → stick 重钉
    Array.from(el.querySelectorAll('.msg')).slice(-3).forEach((n) => n.remove())
    await settle()                             // scroll 事件落地 → 守卫裁决
    snap('after_shrink')
    const afterShrink = { follow: c.follow.value, pinned: dist(el) <= 1 }
    addMsgs(3); await settle()                 // 症状臂：再增长还能不能贴底
    snap('after_regrow')
    return { afterShrink, afterRegrow: { follow: c.follow.value, pinned: dist(el) <= 1 }, trace }
  }

  if (scenario === 'display_none_shrink') {
    addMsgs(7); await settle()                 // 钉底
    snap('pinned')
    // v-show 隐藏（steer 注记行同款）：display:none 收缩，不发 childList，
    // 只有 clamp 产生的 scroll 事件
    el.querySelectorAll('.msg')[6].style.display = 'none'
    await settle()
    snap('after_hide')
    return { afterHide: { follow: c.follow.value, pinned: dist(el) <= 1 }, trace }
  }

  if (scenario === 'hook_round') {
    addMsgs(4); await settle()                  // 4×96=384 基底
    const live = addLive(); await settle()      // live 块挂载（+96 → 480，溢出 80）
    const details = []
    for (let i = 0; i < 3; i++) {               // live 增长 3 拍（每拍都该贴底）
      const d = document.createElement('div'); d.className = 'detail'; live.appendChild(d); details.push(d)
      await settle(); snap('live_tick_' + i)
      if (dist(el) > 1) return { fail: 'live_tick_' + i + ' 未贴底', trace }
    }
    // 轮中折叠（rounds 收起两格 -96px）+ 尾窗增量（+48px）→ 净 -48px，
    // 同一同步块内（镜像 WorkTimelineBlock 真实行为）
    details[0].remove(); details[1].remove()
    const tail = document.createElement('div'); tail.className = 'detail'; live.appendChild(tail)
    await settle()
    snap('midround_fold')
    const midRound = { follow: c.follow.value, pinned: dist(el) <= 1 }
    // 轮末：flush live（整棵卸载 -288）+ 落地折叠消息（+96）→ 净 -192，
    // 同一同步块（镜像 flushPendingWork + addMessage）
    live.remove()
    addMsg()
    await settle()
    snap('round_end')
    const roundEnd = { follow: c.follow.value, pinned: dist(el) <= 1 }
    // 下一轮：live 再挂载 + 增长——验收标准 = 内容贴底可见
    const live2 = addLive()
    const d2 = document.createElement('div'); d2.className = 'detail'; live2.appendChild(d2)
    await settle()
    snap('next_round')
    return { midRound, roundEnd,
      nextRound: { follow: c.follow.value, pinned: dist(el) <= 1 }, trace }
  }

  if (scenario === 'yank_semantics') {
    addMsgs(7); await settle()                  // 钉底跟随（11-25 起点形态）
    el.dispatchEvent(new WheelEvent('wheel', { deltaY: -30 }))
    const wheelDisarm = c.follow.value === false
    el.scrollTop -= 30; await settle()          // 圈内采样（距底 30 ≤ 80）
    const inCircleNoRearm = c.follow.value === false
    addMsgs(2); await settle()                  // 内容增长
    const noYank = dist(el) > 150               // 不拽底：上滚净位移保留
    const stBefore = el.scrollTop
    el.scrollTop = el.scrollHeight; await settle() // 滚回底部
    const rearmed = c.follow.value === true
    addMsgs(1); await settle()
    const followWorks = dist(el) <= 1
    return { wheelDisarm, inCircleNoRearm, noYank, rearmed, followWorks,
      stBefore, trace }
  }

  if (scenario === 'intent_still_kills') {
    addMsgs(7); await settle()
    el.scrollTop -= 200; await settle()         // 用户上滚（无几何变化）
    const intentKills = c.follow.value === false
    addMsgs(2); await settle()
    const staysPut = dist(el) > 150
    return { intentKills, staysPut, trace }
  }

  throw new Error('unknown scenario ' + scenario)
}
`

// --- 5. 驱动 + 断言 ----------------------------------------------------------
const mods = oldSrc ? ['StickModuleOld', 'StickModuleNew'] : ['StickModuleNew']
const scenarios = ['shrink_repin', 'display_none_shrink', 'hook_round', 'yank_semantics', 'intent_still_kills']

const browser = await chromium.launch()
const page = await (await browser.newContext({ viewport: { width: 800, height: 600 } })).newPage()
const pageErrors = []
page.on('pageerror', (e) => pageErrors.push(String(e)))
await page.goto('file:///' + path.join(TMP, 'index.html').replace(/\\/g, '/'))
await page.evaluate(pageSource)

const results = {}
for (const mod of mods) {
  results[mod] = {}
  for (const sc of scenarios) {
    if (mod === 'StickModuleOld' && (sc === 'yank_semantics' || sc === 'intent_still_kills')) continue
    results[mod][sc] = await page.evaluate(([m, s]) => window.runScenario(m, s), [mod, sc])
  }
}
await browser.close()

const failures = []
const check = (cond, label, detail) => {
  const mark = cond ? 'PASS' : 'FAIL'
  if (!cond) failures.push(`${label} :: ${detail}`)
  console.log(`  [${mark}] ${label}${cond ? '' : '  ← ' + detail}`)
}

console.log('\n=== 场景 1：收缩重钉（轮末 live 卸载换装，-288px）===')
if (results.StickModuleOld) {
  const o = results.StickModuleOld.shrink_repin
  check(o.afterShrink.follow === false,
    '旧版复现 BUG（follow 被几何重钉误杀；pinned=true 是 stick 先重钉、scroll 事件后杀的正常中间态）',
    `afterShrink=${JSON.stringify(o.afterShrink)} afterRegrow=${JSON.stringify(o.afterRegrow)}`)
  check(o.afterRegrow.follow === false || !o.afterRegrow.pinned,
    '旧版复现症状（再增长不贴底 = HOOK 内容堆积视口下）',
    `afterRegrow=${JSON.stringify(o.afterRegrow)}`)
}
{
  const n = results.StickModuleNew.shrink_repin
  check(n.afterShrink.follow === true && n.afterShrink.pinned,
    '新版：收缩重钉判几何，follow 存活',
    `afterShrink=${JSON.stringify(n.afterShrink)}`)
  check(n.afterRegrow.follow === true && n.afterRegrow.pinned,
    '新版：后续增长照常贴底（用户验收标准）',
    `afterRegrow=${JSON.stringify(n.afterRegrow)}`)
}

console.log('\n=== 场景 2：v-show 隐藏收缩（无 childList，纯 clamp）===')
if (results.StickModuleOld) {
  const o = results.StickModuleOld.display_none_shrink
  check(o.afterHide.follow === false,
    '旧版复现 BUG（clamp 杀 follow）', `afterHide=${JSON.stringify(o.afterHide)}`)
}
{
  const n = results.StickModuleNew.display_none_shrink
  check(n.afterHide.follow === true && n.afterHide.pinned,
    '新版：clamp 判几何，follow 存活且仍钉底',
    `afterHide=${JSON.stringify(n.afterHide)}`)
}

console.log('\n=== 场景 3：HOOK 全轮序列（live 增长→轮中折叠→轮末换装→下一轮）===')
if (results.StickModuleOld) {
  const o = results.StickModuleOld.hook_round
  if (!o.midRound || o.midRound.follow !== false) {
    console.log('  旧版 trace:', JSON.stringify(o.trace))
  }
  check(o.midRound.follow === false,
    '旧版复现 BUG（轮中折叠即杀 follow，不止轮末）',
    `midRound=${JSON.stringify(o.midRound)}`)
}
{
  const n = results.StickModuleNew.hook_round
  const all = { ...n.midRound, ...n.roundEnd, ...n.nextRound }
  check(n.midRound.follow && n.midRound.pinned,
    '新版：轮中折叠不杀 follow 且贴底', `midRound=${JSON.stringify(n.midRound)}`)
  check(n.roundEnd.follow && n.roundEnd.pinned,
    '新版：轮末换装（live 卸载+落地）不杀 follow 且贴底', `roundEnd=${JSON.stringify(n.roundEnd)}`)
  check(n.nextRound.follow && n.nextRound.pinned,
    '新版：下一轮 live 增长照常贴底可见', `nextRound=${JSON.stringify(n.nextRound)}`)
  if (!all.follow || !all.pinned) console.log('  trace:', JSON.stringify(n.trace, null, 1))
}

console.log('\n=== 场景 4：11-25 拽底循环语义保持（仅新版）===')
{
  const n = results.StickModuleNew.yank_semantics
  check(n.wheelDisarm, '滚轮上滚即时放弃跟随', JSON.stringify(n))
  check(n.inCircleNoRearm, '圈内采样不重臂（11-25 语义）', JSON.stringify(n))
  check(n.noYank, '增长不拽底（上滚净位移保留）', `dist=${'见前'}`)
  check(n.rearmed && n.followWorks, '滚回底部恢复跟随且贴底生效', JSON.stringify(n))
}

console.log('\n=== 场景 5：用户上滚仍判意图（仅新版）===')
{
  const n = results.StickModuleNew.intent_still_kills
  check(n.intentKills && n.staysPut, '真实上滚杀 follow 且位置不被拽走', JSON.stringify(n))
}

if (pageErrors.length) failures.push('页面错误: ' + pageErrors.join(' | '))

console.log('\n' + (failures.length
  ? `✗ ${failures.length} 项不符合预期`
  : '✓ 全部符合预期：旧版复现 BUG、新版修复成立、历史语义零回退'))
if (failures.length) { for (const f of failures) console.log('  - ' + f); process.exitCode = 1 }
