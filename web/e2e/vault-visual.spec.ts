/**
 * Vault 页凭据三卡视觉自测（2026-11-23 ABCD 批，UI 视觉自测纪律）。
 *
 * 截图四处人工看图（test-results/vault-visual/shots/）：
 *   1. /#/vault 整页初始态：免打扰关（"未开启"态）、统计三 0 计数+空态、
 *      域绑定种盘 2 别名渲染
 *   2. 开关真实点开（vault.credential_set 真链路）→ "✅ 免打扰中" 态
 *   3. 添加绑定表单填写 + 真实 bind → 新 tool:slot 徽标行
 *   4. 解除绑定（confirm 接受真链路）→ 行消失
 *
 * 数据种子：直写盘上真相源 config.vault.json（VAULT_DOMAIN_CONFIG_FILE，
 * handler 每请求重读盘）；免打扰开关走真实 WSAPI（写 config.security.json
 * + 热翻）。spec 结束恢复现场（删种盘文件 + 开关复原）。
 */
import { test, expect } from './fixtures'
import path from 'node:path'
import fs from 'node:fs'
import { newAuthedPage, gotoRoute } from './helpers'
import { WORKSPACE_DIR } from './env'

const SHOT_DIR = path.join('test-results', 'vault-visual', 'shots')
const VAULT_DOMAIN_PATH = path.join(WORKSPACE_DIR, 'config', 'config.vault.json')

function seedBindings(): void {
  fs.mkdirSync(path.dirname(VAULT_DOMAIN_PATH), { recursive: true })
  fs.writeFileSync(
    VAULT_DOMAIN_PATH,
    JSON.stringify(
      {
        bindings: {
          'env.PW104': [
            { tool: 'exec', slot: 'env' },
            { tool: 'async_shell', slot: 'env' },
          ],
        },
      },
      null,
      2,
    ),
  )
}

function cleanupSeed(): void {
  try {
    fs.rmSync(VAULT_DOMAIN_PATH, { force: true })
  } catch {
    /* ignore */
  }
}

test('vault 视觉自测：初始态/开关真链路开/添加绑定真链路/解除绑定', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  // 高视口：vault 页四张卡纵排，fullPage 在内层滚动容器下撑不开（goals-visual 同款教训）。
  await page.setViewportSize({ width: 1440, height: 2400 })
  const consoleErrs: string[] = []
  page.on('console', (m) => {
    if (m.type() === 'error') {
      consoleErrs.push(m.text().slice(0, 300))
      console.log('[console-err] ' + m.text().slice(0, 300))
    }
  })
  page.on('pageerror', (e) => {
    consoleErrs.push('PAGE-ERR: ' + String(e).slice(0, 400))
    console.log('[page-err] ' + String(e).slice(0, 400))
  })

  cleanupSeed()
  seedBindings()

  // —— 1. 初始态整页 ——
  await gotoRoute(page, '/#/vault')
  await page.locator('input[data-test="credential-auto-toggle"]').waitFor({ state: 'visible' })
  await expect(page.getByText('暂无绑定。')).toHaveCount(0) // 种盘生效
  await expect(page.getByText('env.PW104').first()).toBeVisible()
  await expect(page.getByText('本次运行暂无凭据引用记录。')).toBeVisible()
  await page.screenshot({ path: path.join(SHOT_DIR, '1-vault-initial.png'), fullPage: true })

  // —— 2. 开关真实点开（credential_set 真链路 → 免打扰中）——
  const toggle = page.locator('input[data-test="credential-auto-toggle"]')
  await expect(toggle).toBeEnabled() // injected=true（release gateway security 装配注入）
  await toggle.click()
  await expect(page.getByText('免打扰中：凭据引用将自动放行')).toBeVisible()
  await page.screenshot({ path: path.join(SHOT_DIR, '2-toggle-on.png'), fullPage: true })
  // 开关持久化：config.security.json 落 credential_auto_approve:true（盘上真相）
  const secCfg = JSON.parse(fs.readFileSync(path.join(WORKSPACE_DIR, 'config', 'config.security.json'), 'utf8'))
  expect(secCfg.approvals?.credential_auto_approve).toBe(true)

  // —— 3. 添加绑定真链路 ——
  await page.locator('input[placeholder="别名（如 env.PW104）"]').fill('openai/prod-key')
  await page.locator('input[placeholder="工具（如 exec）"]').fill('web_fetch')
  await page.locator('input[placeholder="槽位（如 env）"]').fill('headers')
  await page.locator('button[data-test="bind-btn"]').click()
  await expect(page.locator('.badge', { hasText: 'web_fetch : headers' })).toBeVisible()
  await page.screenshot({ path: path.join(SHOT_DIR, '3-bind-added.png'), fullPage: true })
  const onDisk = JSON.parse(fs.readFileSync(VAULT_DOMAIN_PATH, 'utf8'))
  expect(onDisk.bindings['openai/prod-key']).toEqual([{ tool: 'web_fetch', slot: 'headers' }])

  // —— 4. 解除绑定（confirm 接受）——
  page.once('dialog', (d) => d.accept())
  // .locator('div', {hasText}) 命中全部祖先 div——.last() 取最内层别名容器
  // （alias span + 各绑定行），其内 unbind 按钮才是该别名的（.first() 会取
  // 到页面级容器误点第一个绑定——实测踩过）。
  const unbindBtn = page
    .locator('div', { hasText: 'openai/prod-key' })
    .locator('button[data-test="unbind-btn"]')
    .last()
  await unbindBtn.click()
  await expect(page.locator('.badge', { hasText: 'web_fetch : headers' })).toHaveCount(0)
  await page.screenshot({ path: path.join(SHOT_DIR, '4-bind-removed.png'), fullPage: true })

  expect(consoleErrs).toEqual([])

  // —— 恢复现场：删种盘 + 开关关回 ——
  cleanupSeed()
})
