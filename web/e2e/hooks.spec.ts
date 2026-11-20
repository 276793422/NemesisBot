/**
 * e2e/hooks.spec.ts — HookView（hooks.json 编辑器 + 会话圈定）真实浏览器全链路。
 *
 * 固化自 2026-11-17 CDP E2E 实录（Temp drive_ui.mjs 15 断言 + drive_ui2.mjs
 * 2 断言，两段全 PASS；见 docs/REPORT/2026-11-17_hook会话圈定交付报告.md
 * 「UI 真实浏览器 E2E 实录」节），按仓库 Playwright 惯例迁入可复跑套件。
 *
 * 两段：
 * 1) 编辑面：设置 TAB 12 事件卡 → PreToolUse 添加钩子 → 作用会话选择器
 *    （sessions.list 真实数据）→ 勾选圈定 → 保存 → WSAPI hooks.get 落盘
 *    验证（嵌套方言 group.hooks[0].command + scope.sessions）→ 总览作用域
 *    徽标（.hooks-detail-scope 单会话=标题）→ 原文 TAB 手写 scope 往返 →
 *    结构化 TAB 读回（空数组 scope = 全部会话投影）。
 * 2) 运行时：即刻生效语义（2026-11-20 冷启动修复——gateway 启动时 hooks.json
 *    缺失 = 待命桥，load_from_dir 恒建桥；首次保存**不重启**直接触发）→
 *    UI 保存的圈定钩子（UserPromptSubmit）在圈定会话实弹触发（marker 落盘）、
 *    他话零触发（圈定排除）→ restartGateway 后同钩子仍触发（启动装载持久化）。
 *
 * 实弹载体用 UserPromptSubmit（任意用户消息触发，零工具零审批——mock LLM
 * 的 exec 路由会撞审批 ask 规则引入无关交互），场景等价于实录里的 PreToolUse
 * 全链（同一桥、同一圈定过滤路径、同一 marker 观测面）。
 */
import fs from 'node:fs'
import path from 'node:path'
import { expect, test, type Page } from './fixtures'
import { WORKSPACE_DIR } from './env'
import { newAuthedPage, gotoRoute, restartGateway, wsapi, wsChatSend } from './helpers'

const MARKER = path.join(WORKSPACE_DIR, 'temp', 'hooks-e2e-marker.txt')
// cmd /C 执行；正斜杠路径 cmd echo 重定向可解析（实录 drive_ui 同形态实证）。
const HOOK_CMD = `echo hooks-e2e-fired >> ${MARKER.replace(/\\/g, '/')}`
const TAG = 'hooks-e2e-fired'

function markerCount(): number {
  try {
    return fs.readFileSync(MARKER, 'utf-8').split(TAG).length - 1
  } catch {
    return 0
  }
}

/** 轮询 marker 到达目标计数（钩子子进程 spawn→写盘有毫秒级窗口）。 */
async function pollMarker(target: number, timeoutMs = 15_000): Promise<number> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const n = markerCount()
    if (n >= target) return n
    if (Date.now() > deadline) return n
    await new Promise((r) => setTimeout(r, 300))
  }
}

async function seedSession(page: Page, title: string): Promise<string> {
  const r = await wsapi(page, 'sessions', 'create', { title })
  expect(r.ok, `sessions.create(${title}) 失败: ${r.error}`).toBe(true)
  return (r.data as { session_id: string }).session_id
}

function makeRaw(sids: string[]): string {
  return JSON.stringify(
    {
      hooks: {
        UserPromptSubmit: [
          { scope: { sessions: sids }, hooks: [{ type: 'command', command: HOOK_CMD }] },
        ],
      },
    },
    null,
    1,
  )
}

test('HookView 编辑面：添加圈定钩子 → 落盘 → 总览徽标 → 原文往返', async ({ browser }) => {
  fs.mkdirSync(path.dirname(MARKER), { recursive: true })
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // 圈定目标需要真实会话（选择器数据源 = sessions.list）。sessions.create
  // 是懒物化（首条消息才落 session_logs jsonl，服务端 list 枚举 jsonl 不含
  // 零消息会话）——种子后各发一条消息物化，选择器才能列出。
  const sidA = await seedSession(page, 'Hooks-E2E-A')
  const sidB = await seedSession(page, 'Hooks-E2E-B')
  await wsChatSend(page, sidA, 'Hooks-E2E-A 物化种子')
  await wsChatSend(page, sidB, 'Hooks-E2E-B 物化种子')

  await gotoRoute(page, '/#/hooks')
  await expect(page.locator('.page-hooks')).toBeVisible({ timeout: 20_000 })

  // T1: 总览/设置双 TAB。
  await expect(page.locator('button.tab', { hasText: '总览' })).toBeVisible()
  await expect(page.locator('button.tab', { hasText: '设置' })).toBeVisible()

  // T2: 设置 TAB 12 事件卡各带「+ 添加钩子」。
  await page.locator('button.tab', { hasText: '设置' }).click()
  await expect(page.locator('.page-hooks button', { hasText: '+ 添加钩子' })).toHaveCount(12)

  // T3: PreToolUse 卡添加一行 → 作用会话按钮缺省「全部会话」。
  const preCard = page.locator('.page-hooks .card', { hasText: 'PreToolUse' }).first()
  await preCard.locator('button', { hasText: '+ 添加钩子' }).click()
  await expect(preCard.locator('.hooks-scope-btn')).toHaveText(/全部会话/)

  // T4: 命令输入可编辑（marker 观测命令）。
  const cmdInput = preCard.locator('input[placeholder^="命令"]').last()
  await cmdInput.fill(HOOK_CMD)
  await expect(cmdInput).toHaveValue(HOOK_CMD)

  // T5: 选择器弹层拉 sessions.list 真实数据（此前 spec 可能已建会话——
  // 断言自有两条在场 + 总行数 ≥2，不锁总数）。
  await preCard.locator('.hooks-scope-btn').click()
  const itemA = page.locator('.hooks-scope-item', { hasText: 'Hooks-E2E-A' })
  const itemB = page.locator('.hooks-scope-item', { hasText: 'Hooks-E2E-B' })
  await expect(itemA).toHaveCount(1)
  await expect(itemB).toHaveCount(1)

  // T6: 勾选圈定目标 → footer 计数「已选 1 个」。
  await itemA.locator('input[type="checkbox"]').check()
  await expect(page.locator('.hooks-scope-picker-footer')).toContainText('已选 1 个')

  // T7: 完成 → 按钮文案联动「已圈 1 个会话」。
  await page.locator('.hooks-scope-picker-footer button', { hasText: '完成' }).click()
  await expect(preCard.locator('.hooks-scope-btn')).toHaveText(/已圈 1 个会话/)

  // T8: 保存全部钩子 → WSAPI hooks.get 硬核对落盘（嵌套方言 + scope）。
  // 保存走 app WS、断言读走 e2e WS 单例——跨连接无读写序，组合跑高负载下
  // 稳定竞速假红（2026-11-18 三连挂三不同断言点；旧 UI 对照复现=既有抖动
  // 非本批引入）。「已保存」toast 信号也不可靠（上一保存的 toast 存活数秒
  // 会被下次保存的等待复用），改 expect.poll 轮询读盘到目标形态——保存真坏
  // = 超时诚实红，不掩盖。
  await page.locator('.page-hooks button', { hasText: '保存全部钩子' }).click()
  await expect
    .poll(
      async () => {
        const g = await wsapi(page, 'hooks', 'get')
        return g.ok ? (g.data as { content: string }).content : ''
      },
      { timeout: 15_000 },
    )
    .toContain(TAG)
  const got = await wsapi(page, 'hooks', 'get')
  expect(got.ok, `hooks.get 失败: ${got.error}`).toBe(true)
  const saved = JSON.parse((got.data as { content: string }).content)
  const preGroups = saved.hooks?.PreToolUse ?? []
  expect(
    preGroups.some(
      (g: { hooks?: { command: string }[]; scope?: { sessions?: string[] } }) =>
        g.hooks?.[0]?.command?.includes(TAG) &&
        Array.isArray(g.scope?.sessions) &&
        g.scope!.sessions!.length === 1 &&
        g.scope!.sessions![0] === sidA,
    ),
    `落盘内容缺命令或 scope: ${JSON.stringify(preGroups)}（期望圈定 ${sidA}）`,
  ).toBe(true)

  // T9: 总览 TAB 作用域徽标（.hooks-detail-scope 单会话=标题本身）。
  await page.locator('button.tab', { hasText: '总览' }).click()
  await expect(page.locator('.page-hooks .hooks-detail-scope', { hasText: 'Hooks-E2E-A' })).toBeVisible()

  // T10: 原文 textarea 含 scope 字段（切 TAB 触发磁盘重载）。
  await expect(page.locator('.page-hooks textarea')).toHaveValue(/"scope"/)

  // T11: 手写原文往返——UserPromptSubmit 空 scope（=全部会话）保存 → 读回。
  const raw = makeRaw([])
  await page.locator('.page-hooks textarea').fill(raw)
  await page.locator('.page-hooks button', { hasText: '保存原文' }).click()
  // 同 T8 跨连接竞速：轮询到原文落盘（T8 落的 sidA 不在原文里，出现 = 新写完成）。
  await expect
    .poll(
      async () => {
        const g = await wsapi(page, 'hooks', 'get')
        return g.ok ? (g.data as { content: string }).content : ''
      },
      { timeout: 15_000 },
    )
    .toContain('"sessions": []')
  const got2 = await wsapi(page, 'hooks', 'get')
  const saved2 = JSON.parse((got2.data as { content: string }).content)
  expect(saved2.hooks?.UserPromptSubmit?.[0]?.scope?.sessions).toEqual([])

  // T12: 切设置 TAB 结构化读回手写条目（空数组 scope → 「全部会话」投影）。
  await page.locator('button.tab', { hasText: '设置' }).click()
  const upsCard = page.locator('.page-hooks .card', { hasText: 'UserPromptSubmit' }).first()
  await expect(upsCard.locator('.hooks-scope-btn')).toHaveText(/全部会话/)

  await page.context().close()
})

test('HookView 即刻生效语义 + 冷启动首保实弹 + 圈定钩子运行时实弹', async ({ browser }) => {
  test.setTimeout(240_000) // restartGateway 预算 150s + 触发轮次
  fs.mkdirSync(path.dirname(MARKER), { recursive: true })
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  const sidC = await seedSession(page, 'Hooks-E2E-C')
  const sidD = await seedSession(page, 'Hooks-E2E-D')
  // 物化（同测试 1：零消息会话不进 sessions.list；此消息发生在 hooks.json
  // 落盘之前 = 空快照零触发，不污染 marker 基线）。
  await wsChatSend(page, sidC, 'Hooks-E2E-C 物化种子')
  await wsChatSend(page, sidD, 'Hooks-E2E-D 物化种子')

  // 原文 TAB 写入 UserPromptSubmit 圈定钩子（覆盖编辑面测试留下的任何配置）。
  await gotoRoute(page, '/#/hooks')
  await expect(page.locator('.page-hooks')).toBeVisible({ timeout: 20_000 })
  await page.locator('.page-hooks textarea').fill(makeRaw([sidC]))
  await page.locator('.page-hooks button', { hasText: '保存原文' }).click()
  // 同 T8 跨连接竞速：轮询到 sidC 落盘（编辑面留下的原文无 sidC）。
  await expect
    .poll(
      async () => {
        const g = await wsapi(page, 'hooks', 'get')
        return g.ok ? (g.data as { content: string }).content : ''
      },
      { timeout: 15_000 },
    )
    .toContain(sidC)
  const got = await wsapi(page, 'hooks', 'get')
  const saved = JSON.parse((got.data as { content: string }).content)
  expect(saved.hooks?.UserPromptSubmit?.[0]?.scope?.sessions).toEqual([sidC])

  // R0 即刻生效（2026-11-20 冷启动修复回归）：gateway 启动时 hooks.json
  // 不存在 = 待命桥（load_from_dir 恒建桥）——首次保存**不重启**直接触发
  // （旧语义在此只落盘、等重启建桥）。触发走 wsChatSend（页面级 token 直开
  // WS），页面停留在 #/hooks 即可。
  const cold0 = markerCount()
  await wsChatSend(page, sidC, 'Hooks-E2E 冷启动即刻触发')
  const cold1 = await pollMarker(cold0 + 1)
  expect(cold1, `冷启动首保未即刻触发（marker ${cold0} -> ${cold1}）`).toBeGreaterThanOrEqual(
    cold0 + 1,
  )

  // R1 其他会话：圈定排除，零触发。
  const other0 = markerCount()
  await wsChatSend(page, sidD, 'Hooks-E2E 他话触发（应被圈定排除）')
  await page.waitForTimeout(2_000)
  expect(markerCount()).toBe(other0)

  // R2 重启持久化：重启后同一 hooks.json 启动装载，钩子仍触发。
  await restartGateway()

  const warm0 = markerCount()
  await wsChatSend(page, sidC, 'Hooks-E2E 重启后触发')
  const warm1 = await pollMarker(warm0 + 1)
  expect(warm1, `重启后 marker 未增长（${warm0} -> ${warm1}）`).toBeGreaterThanOrEqual(warm0 + 1)

  await page.context().close()
})
