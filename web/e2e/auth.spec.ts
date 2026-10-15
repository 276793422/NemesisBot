/**
 * e2e/auth.spec.ts — 认证闸（AuthOverlay）。
 *
 * 1. 无 token → 登录遮罩出现
 * 2. 错误 token → .auth-error 报错（不进主界面）
 * 3. 正确 token → 遮罩消失，主界面渲染
 */
import { expect, test } from '@playwright/test'
import { BASE_URL, E2E } from './env'

test('认证闸：无 token 拦截 / 错误 token 报错 / 正确 token 进入', async ({ browser }) => {
  const context = await browser.newContext()
  const page = await context.newPage()
  await page.goto(BASE_URL)

  // 无 token → 遮罩
  await expect(page.locator('.auth-overlay')).toBeVisible({ timeout: 20_000 })

  // 错误 token → 报错
  const input = page.locator('.form-input').first()
  await input.fill('definitely-wrong-token')
  await page.locator('.btn-primary.btn-lg').click()
  await expect(page.locator('.auth-error')).toBeVisible({ timeout: 20_000 })
  await expect(page.locator('.auth-overlay')).toBeVisible()

  // 正确 token → 进入主界面（输入区 = ChatPanel 已挂载的标志）
  await input.fill(E2E.token)
  await page.locator('.btn-primary.btn-lg').click()
  await expect(page.locator('.auth-overlay')).toBeHidden({ timeout: 20_000 })
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  await context.close()
})
