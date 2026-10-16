/**
 * e2e/model-switch.spec.ts — 模型管理面 + 热切默认模型（WSAPI）。
 *
 * models.list 双 mock 条目可见、is_default 挂在 A（global-setup 写的
 * agents.defaults.llm）→ set_default 切 B → 列表复核 → 切回 A
 * （后续 spec 的默认模型预期恢复一致）。
 */
import { expect, test, type Page } from '@playwright/test'
import { MODEL_A, MODEL_B } from './env'
import { newAuthedPage, wsapi } from './helpers'

interface ModelRow {
  model_name: string
  is_default: boolean
}

async function listModels(page: Page): Promise<ModelRow[]> {
  const res = await wsapi(page, 'models', 'list')
  expect(res.ok, `models.list 失败: ${res.error}`).toBe(true)
  return (res.data as { models: ModelRow[] }).models
}

test('模型热切：list 双条目 → set_default 切 B 复核 → 切回 A', async ({ browser }) => {
  const page = await newAuthedPage(browser)

  // 双条目 + 默认在 A
  const before = await listModels(page)
  expect(before.map((m) => m.model_name)).toEqual(expect.arrayContaining([MODEL_A, MODEL_B]))
  expect(before.find((m) => m.model_name === MODEL_A)?.is_default).toBe(true)
  expect(before.find((m) => m.model_name === MODEL_B)?.is_default).toBe(false)

  // 切 B + 复核
  const sw = await wsapi(page, 'models', 'set_default', { name: MODEL_B })
  expect(sw.ok, `models.set_default 失败: ${sw.error}`).toBe(true)
  const after = await listModels(page)
  expect(after.find((m) => m.model_name === MODEL_B)?.is_default).toBe(true)
  expect(after.find((m) => m.model_name === MODEL_A)?.is_default).toBe(false)

  // 切回 A（后续 spec 假设默认模型 = A）
  const back = await wsapi(page, 'models', 'set_default', { name: MODEL_A })
  expect(back.ok, `切回失败: ${back.error}`).toBe(true)

  await page.context().close()
})
