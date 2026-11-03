/**
 * e2e/proxy-keys.spec.ts — 代理调用方 Key 卡片（/proxy-settings）UI 全链路。
 *
 * 2026-11-03 LLM代理调用方key管理 复测补强（浏览器 UI 实测）：
 *   空态 → UI 添加（key 留空自动生成 nbk_+32hex）→ keys.list 复核 →
 *   持 key 打 bare lane 200（body 带非空 tools → bare forward 到 mock）→
 *   UI 禁用 → lane 即刻 401 → UI 启用 → lane 恢复 200 →
 *   key=主 token 防呆 loud 拒绝 → 白名单外 401 →
 *   UI 删除（confirm 对话框）→ lane 401 + 列表清空。
 *
 * 自包含：本 spec 创建的 key 结束前删除，不影响其他 spec。
 */
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { BASE_URL, E2E, MODEL_A } from './env'
import { newAuthedPage, wsapi } from './helpers'

interface ProxyKeyRow {
  name: string
  key: string
  enabled: boolean
  created_at: number
  note: string
}

interface KeysListPayload {
  file_exists: boolean
  path: string
  keys: ProxyKeyRow[]
}

async function listKeys(page: Page): Promise<KeysListPayload> {
  const res = await wsapi(page, 'proxy', 'keys.list')
  expect(res.ok, `keys.list 失败: ${res.error}`).toBe(true)
  return res.data as KeysListPayload
}

/** bare lane 请求体：tools 非空才走 bare forward（openai_compat 路由条件）。 */
const BARE_BODY = {
  model: MODEL_A,
  messages: [{ role: 'user', content: 'e2e proxy-key lane ping' }],
  tools: [
    {
      type: 'function',
      function: { name: 'noop', description: 'no-op', parameters: { type: 'object', properties: {} } },
    },
  ],
}

async function laneStatus(request: APIRequestContext, key: string): Promise<number> {
  const res = await request.post(`${BASE_URL}/v1/chat/completions`, {
    headers: { Authorization: `Bearer ${key}` },
    data: BARE_BODY,
    timeout: 30_000,
  })
  return res.status()
}

test('代理调用方 Key：UI 添加/启停/删除全链路 + lane 热生效', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await page.goto(`${BASE_URL}/#/proxy-settings`)

  // 卡片在页面 + 空态（e2e home 无 config.proxy_keys.json）
  const card = page.locator('.card', { hasText: '代理调用方 Key' })
  await expect(card).toBeVisible()
  await expect(card.getByText('尚未签发任何调用方 key', { exact: false })).toBeVisible()

  // 空名称 = 添加按钮禁用（客户端预检）
  await expect(card.getByRole('button', { name: '添加', exact: true })).toBeDisabled()

  // UI 添加：key 留空自动生成
  await card.getByPlaceholder('调用方名称（必填）').fill('e2e-ui-bot')
  await card.getByPlaceholder('备注（可选）').fill('UI 复测')
  await card.getByRole('button', { name: '添加', exact: true }).click()
  const row = page.locator('tr', { hasText: 'e2e-ui-bot' })
  await expect(row).toBeVisible()
  const keyText = (await row.locator('td span.mono').first().textContent()) ?? ''
  expect(keyText).toMatch(/^nbk_[0-9a-f]{32}$/)
  await expect(row.locator('.badge')).toHaveText('启用')

  // WSAPI 复核：落盘一致 + created_at 补戳
  const listed = await listKeys(page)
  expect(listed.file_exists).toBe(true)
  expect(listed.path).toContain('config.proxy_keys.json')
  expect(listed.keys).toHaveLength(1)
  expect(listed.keys[0].name).toBe('e2e-ui-bot')
  expect(listed.keys[0].enabled).toBe(true)
  expect(listed.keys[0].created_at).toBeGreaterThan(0)

  // 持 key 打 bare lane → 200（UI 签发的 key 真实放行）
  expect(await laneStatus(page.request, keyText)).toBe(200)

  // UI 禁用 → 徽标翻禁用 → lane 即刻 401（热生效，无重启）
  await row.getByRole('button', { name: '禁用', exact: true }).click()
  await expect(row.locator('.badge')).toHaveText('禁用')
  expect(await laneStatus(page.request, keyText)).toBe(401)

  // UI 启用 → lane 恢复 200
  await row.getByRole('button', { name: '启用', exact: true }).click()
  await expect(row.locator('.badge')).toHaveText('启用')
  expect(await laneStatus(page.request, keyText)).toBe(200)

  // key = 主 token → 服务端 loud 拒绝（toast 报错可见，列表不变）
  await card.getByPlaceholder('调用方名称（必填）').fill('evil-bot')
  await card.getByPlaceholder('key（留空自动生成）').fill(E2E.token)
  await card.getByRole('button', { name: '添加', exact: true }).click()
  await expect(page.getByText('key 不能与主 token 相同', { exact: false })).toBeVisible()
  expect((await listKeys(page)).keys).toHaveLength(1)

  // key 打白名单外 → 401（放行面只有两条代理 lane）
  const oob = await page.request.get(`${BASE_URL}/api/usage/summary`, {
    headers: { Authorization: `Bearer ${keyText}` },
  })
  expect(oob.status()).toBe(401)

  // UI 删除（confirm 对话框接受）→ 行消失 + lane 401 + 列表清空
  page.once('dialog', (d) => d.accept())
  await row.getByRole('button', { name: '删除', exact: true }).click()
  await expect(row).toBeHidden()
  expect(await laneStatus(page.request, keyText)).toBe(401)
  const after = await listKeys(page)
  expect(after.keys).toHaveLength(0)

  await page.context().close()
})
