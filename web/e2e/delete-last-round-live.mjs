/**
 * delete-last-round-live.mjs — 用户真实 home：删除【测试】会话最后一轮，全量捕获弹出的每一样东西。
 * 捕获面：window.confirm/alert 对话框全文、.toast 逐秒快照、console 错误、WS 响应帧。
 * 用法：node web/e2e/delete-last-round-live.mjs <sidebarDiscriminator> <assistantRowText>
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
const sidebarDiscriminator = process.argv[2]
const assistantRowText = process.argv[3]
const SHOT_DIR = path.join(repoRoot, 'target', 'live-delete-test')
fs.mkdirSync(SHOT_DIR, { recursive: true })

const dialogs = []
const toastsTimeline = [] // {t, texts[]}
const consoleErrors = []

const browser = await chromium.launch()
const context = await browser.newContext({ viewport: { width: 1600, height: 950 } })
await context.addInitScript(([k, token]) => {
  try { localStorage.setItem(k, token) } catch {}
}, ['nemesisbot_auth_token', TOKEN])
const page = await context.newPage()
page.on('console', (m) => { if (m.type() === 'error') consoleErrors.push(m.text()) })
page.on('dialog', async (d) => {
  dialogs.push({ type: d.type(), message: d.message() })
  console.log(`[DIALOG #${dialogs.length}] type=${d.type()} message=<<<${d.message()}>>>`)
  await d.accept()
})

/** toast 快照（当前 DOM 里所有 .toast 文本）。 */
async function snapToasts(tag) {
  const texts = await page.evaluate(() =>
    Array.from(document.querySelectorAll('.toast')).map((el) => {
      const title = el.querySelector('.toast-title')?.textContent?.trim() ?? ''
      const msg = el.querySelector('.toast-message')?.textContent?.trim() ?? ''
      return `${title}: ${msg}`
    }),
  ).catch(() => [])
  if (texts.length) {
    toastsTimeline.push({ tag, texts })
    for (const t of texts) console.log(`[TOAST @${tag}] ${t}`)
  }
}

await page.goto(BASE_URL)
await page.waitForTimeout(3000)
const newChat = page.locator('.fn-new-chat')
if (!(await newChat.isVisible().catch(() => false))) {
  await page.locator('.toolbar-toggle[title="显示会话列表"]').first().click()
  await newChat.waitFor({ state: 'visible', timeout: 15000 })
}
await page.waitForTimeout(800)

// 打开目标会话
const sidebarRow = page.locator('.session-item').filter({ hasText: sidebarDiscriminator }).first()
if (!(await sidebarRow.isVisible().catch(() => false))) {
  console.log('[fail] 侧栏行未找到')
  process.exit(2)
}
await sidebarRow.click()
await page.waitForTimeout(2500)
await page.screenshot({ path: path.join(SHOT_DIR, '10-session-open.png') })

// 定位最后一轮的 assistant 行
const msgRow = page.locator('.msg-menu-trigger').locator('xpath=ancestor::div[contains(@class,"message ")][1]')
const rowCount = await msgRow.count()
let targetIdx = -1
for (let i = rowCount - 1; i >= 0; i--) {
  const t = (await msgRow.nth(i).innerText().catch(() => '')).slice(0, 400)
  if (t.includes(assistantRowText)) { targetIdx = i; break }
}
console.log(`[rows] ${rowCount}, [target idx] ${targetIdx} (从尾往前找)`)
if (targetIdx < 0) { console.log('[fail] 目标行未找到'); process.exit(3) }

const row = msgRow.nth(targetIdx)
await row.scrollIntoViewIfNeeded()
await row.hover()
await page.waitForTimeout(400)
await row.locator('.msg-menu-trigger').first().click()
await page.waitForTimeout(400)
await page.screenshot({ path: path.join(SHOT_DIR, '11-menu-open.png') })

const item = page.getByText(process.env.MENU_LABEL ?? '⏪ 删除整轮', { exact: false }).last()
if (!(await item.isVisible().catch(() => false))) {
  console.log('[fail] 菜单项不可见')
  await page.screenshot({ path: path.join(SHOT_DIR, '12-menu-missing.png') })
  process.exit(4)
}
await item.click()
console.log('[clicked] 删除整轮 — 开始 10s 全量观测')

// 10 秒逐秒观测 toast + 对话框（对话框由 page.on('dialog') 记录）
for (let s = 1; s <= 10; s++) {
  await page.waitForTimeout(1000)
  await snapToasts(`${s}s`)
}
await page.screenshot({ path: path.join(SHOT_DIR, '13-after-click.png'), fullPage: true })

// 刷新持久化核
await page.reload()
await page.waitForTimeout(3000)
const afterText = await page.locator('body').innerText()
console.log('[after reload still contains rowText]', afterText.includes(assistantRowText))
await page.screenshot({ path: path.join(SHOT_DIR, '14-after-reload.png'), fullPage: true })

console.log('==== SUMMARY ====')
console.log('[dialogs]', JSON.stringify(dialogs, null, 1))
console.log('[toast timeline]', JSON.stringify(toastsTimeline, null, 1))
console.log('[console errors]', JSON.stringify(consoleErrors.slice(0, 10)))
await browser.close()
console.log('DONE')
