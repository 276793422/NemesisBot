/**
 * delete-round-live-test.mjs — 用户真实 home（C:\AI\NemesisBot）UI 删一轮实测。
 * 直连已运行的 gateway（E2E_SKIP_LAUNCH 形态），不依赖 e2e global-setup。
 * 用法：node web/e2e/delete-round-live-test.mjs <sessionId> <rowTextSubstr> <menuItem>
 */
import path from 'node:path'
import fs from 'node:fs'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')
const require = createRequire(path.join(repoRoot, 'web', 'package.json'))
const { chromium } = require('playwright')

const BASE_URL = process.env.LIVE_BASE_URL ?? 'http://127.0.0.1:49000'
const TOKEN = process.env.LIVE_TOKEN ?? '276793422'
const AUTH_KEY = 'nemesisbot_auth_token'

const sessionId = process.argv[2]
const sidebarTitle = process.argv[3]
const rowText = process.argv[4]
const menuItem = process.argv[5] ?? '⏪ 删除整轮'
const SHOT_DIR = path.join(repoRoot, 'target', 'live-delete-test')
fs.mkdirSync(SHOT_DIR, { recursive: true })

/** WSAPI 调用（页面内原生 WebSocket，reqId 关联）。 */
async function wsapi(page, module_, cmd, data) {
  return page.evaluate(
    async ([mod, c, d, token]) => {
      const ws = new WebSocket(`ws://${location.host}/ws?token=${encodeURIComponent(token)}`)
      await new Promise((res, rej) => {
        ws.onopen = res
        ws.onerror = () => rej(new Error('ws open failed'))
      })
      const reqId = 'live' + Math.random().toString(36).slice(2)
      const p = new Promise((res) => {
        ws.onmessage = (ev) => {
          const m = JSON.parse(ev.data)
          if (m.reqId === reqId) res(m)
        }
      })
      ws.send(JSON.stringify({ type: 'request', module: mod, cmd: c, data: d, reqId }))
      const r = await Promise.race([p, new Promise((res) => setTimeout(() => res({ timeout: true }), 15000))])
      ws.close()
      return r
    },
    [module_, cmd, data, TOKEN],
  )
}

const browser = await chromium.launch()
const context = await browser.newContext({ viewport: { width: 1600, height: 950 } })
await context.addInitScript(
  ([key, token]) => {
    try {
      localStorage.setItem(key, token)
    } catch {}
  },
  [AUTH_KEY, TOKEN],
)
const page = await context.newPage()
const consoleErrors = []
page.on('console', (m) => {
  if (m.type() === 'error') consoleErrors.push(m.text())
})
// window.confirm（非末轮确认/冲突覆盖确认）一律接受——测试语义要走到后端。
page.on('dialog', (d) => {
  console.log('[dialog]', d.type(), d.message().slice(0, 200))
  d.accept()
})

await page.goto(BASE_URL)
await page.waitForTimeout(3000)
// 侧栏抽屉：未开则点工具栏「显示会话列表」。
const newChat = page.locator('.fn-new-chat')
if (!(await newChat.isVisible().catch(() => false))) {
  await page.locator('.toolbar-toggle[title="显示会话列表"]').first().click()
  await newChat.waitFor({ state: 'visible', timeout: 15000 })
}
await page.waitForTimeout(800)
await page.screenshot({ path: path.join(SHOT_DIR, '01-loaded.png') })

// ---- 会话列表（WSAPI sessions.list）----
const listResp = await wsapi(page, 'sessions', 'list', {})
console.log('[sessions.list]', JSON.stringify(listResp).slice(0, 600))

// ---- 打开目标会话 ----
const sidebarRow = page.locator('.session-item').filter({ hasText: sidebarTitle }).first()
const rowVisible = await sidebarRow.isVisible().catch(() => false)
console.log('[sidebar row visible]', rowVisible)
if (rowVisible) {
  await sidebarRow.click()
} else {
  console.log('[fail] 侧栏行未找到；列出全部会话标题供排查')
  const titles = await page.locator('.session-item .session-title').allInnerTexts()
  console.log('[titles]', JSON.stringify(titles))
  process.exit(2)
}
await page.waitForTimeout(2500)
await page.screenshot({ path: path.join(SHOT_DIR, '02-session-open.png') })

// ---- 找目标行：历史批次里含 rowText 的消息气泡 ----
// 行结构：div.message.user/.assistant（avatar+content+bubble+message-time[⋯触发点]）。
// ancestor 用带尾空格的 "message " 匹配，避开内部的 .message-time 子条。
const msgRow = page.locator('.msg-menu-trigger').locator('xpath=ancestor::div[contains(@class,"message ")][1]')
const allTriggers = page.locator('.msg-menu-trigger')
const triggerCount = await allTriggers.count()
console.log('[menu triggers]', triggerCount)

// 定位含目标文本的气泡所在行：遍历触发点，取其祖先行文本匹配。
let targetIdx = -1
const rowCount = await msgRow.count()
for (let i = 0; i < rowCount; i++) {
  const t = (await msgRow.nth(i).innerText().catch(() => '')).slice(0, 400)
  if (t.includes(rowText)) {
    targetIdx = i
    break
  }
}
console.log('[target row idx]', targetIdx)
if (targetIdx < 0) {
  console.log('[fail] 目标行未找到（rowText=', rowText, '）；dump 实际渲染行：')
  const dumpCount = await msgRow.count()
  console.log('[rendered rows]', dumpCount)
  for (let i = 0; i < dumpCount; i++) {
    const cls = await msgRow.nth(i).getAttribute('class').catch(() => '')
    const t = (await msgRow.nth(i).innerText().catch(() => '')).replace(/\s+/g, ' ').slice(0, 120)
    console.log(`  [row ${i}] cls=${cls} :: ${t}`)
  }
  process.exit(3)
}

// hover 展开行动作组 → 点 ⋯ → 点菜单项
const row = msgRow.nth(targetIdx)
await row.scrollIntoViewIfNeeded()
await row.hover()
await page.waitForTimeout(400)
const trigger = row.locator('.msg-menu-trigger').first()
await trigger.click()
await page.waitForTimeout(400)
await page.screenshot({ path: path.join(SHOT_DIR, '03-menu-open.png') })

const item = page.getByText(menuItem, { exact: false }).last()
const itemVisible = await item.isVisible().catch(() => false)
console.log('[menu item visible]', itemVisible)
if (!itemVisible) {
  const bodyText = await page.locator('body').innerText()
  console.log('[menu area text]', bodyText.slice(0, 1500))
  process.exit(4)
}
await item.click()
await page.waitForTimeout(4000) // rewind + resync 窗口
await page.screenshot({ path: path.join(SHOT_DIR, '04-after-click.png'), fullPage: true })

// toast 抓取
const bodyText = await page.locator('body').innerText()
const toastLines = bodyText.split('\n').filter((l) => l.includes('回退') || l.includes('删除') || l.includes('不可用') || l.includes('失败') || l.includes('未执行'))
console.log('[toast-ish lines]', JSON.stringify(toastLines.slice(0, 12)))

// ---- 刷新后状态（持久化核）----
await page.reload()
await page.waitForTimeout(3000)
await page.screenshot({ path: path.join(SHOT_DIR, '05-after-reload.png'), fullPage: true })
const afterText = await page.locator('body').innerText()
console.log('[after reload contains rowText]', afterText.includes(rowText))
console.log('[console errors]', JSON.stringify(consoleErrors.slice(0, 10)))

await browser.close()
console.log('DONE')
