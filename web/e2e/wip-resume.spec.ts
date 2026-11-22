/**
 * e2e/wip-resume.spec.ts — wip 侧车真实链路验收（2026-11-22 批二 A）
 *
 * 回答「针对我们点的测试」：A 的验收口径是真实 gateway 崩溃链——turn 中途
 * 硬杀 gateway 进程（taskkill /F /T，非优雅停机）→ 重启 → 用户输入「继续」
 * → 中断现场注入进**真实 LLM 请求**。loop 级单测（wip_turn_tests，mock
 * provider 录制）已验证机制咬合；本 spec 用 verify-mock-llm 的证据面
 * （wip_scene 旗标 + 完整请求 dump）证明注入到达 mock 收到的请求体。
 *
 * 链路：
 *  1. UI 建会话，sendChat(E2E_SLOW_TOOL ...) → mock 延迟 SLOW_TOOL_MS 后
 *     应答 sleep 工具调用（秒数 = SLOW_TOOL_SLEEP_SECS，跑本 spec 时设 12
 *     拉宽「tool_start 已落盘 → sleep 进行中」的硬杀窗口）
 *  2. 轮询 <workspace>/logs/wip/*.jsonl 出现 sleep 的 tool_start（append 是
 *     per-event open+write_all 直写、无用户态缓冲——落盘内容硬杀安全）
 *  3. restartGateway()（supervisor taskkill /F /T 硬杀 + 重拉）= 真崩溃形态
 *  4. reload 页面 → 重选会话 → sendChat('继续')
 *  5. 断言：
 *     - 负对照：kill 前所有 mock 请求 wip_scene=false（现场无遗留可注入）
 *     - 「继续」的请求 wip_scene=true（注入到达）
 *     - mock-evidence.last-request.json 含 'Interrupted Turn Scene' 节 +
 *       被杀 turn 的 sleep 参数（"seconds":12——被杀 turn 未收尾、assistant
 *       行未落盘，该参数只可能来自 wip 注入节，唯一性成立）
 *     - 「继续」回复落地后 <workspace>/logs/wip/ 清空（fold 折叠收口）
 */
import fs from 'node:fs'
import path from 'node:path'
import { expect, test } from './fixtures'
import { EVIDENCE_LAST_REQUEST, WORKSPACE_DIR } from './env'
import {
  evidenceCount,
  evidenceLines,
  newAuthedPage,
  openSessionSidebar,
  restartGateway,
  sendChat,
  waitAssistantReply,
} from './helpers'

// 与 mock 侧同名 env **缺省值必须同步**（mock = SLOW_TOOL_SLEEP_SECS ?? 3）：
// 断言的 seconds 值 = mock 实际下发的 sleep 参数，裸跑（无 env）时两端都是
// 3；要拉宽硬杀窗口就两端一起拉（env 经 supervisor 继承进 mock 进程），
// 如 SLOW_TOOL_SLEEP_SECS=12。
const SLEEP_SECS = Number(process.env.SLOW_TOOL_SLEEP_SECS ?? '3')
const WIP_DIR = path.join(WORKSPACE_DIR, 'logs', 'wip')

function wipFiles(): string[] {
  try {
    return fs.readdirSync(WIP_DIR).filter((f) => f.endsWith('.jsonl'))
  } catch {
    return []
  }
}

function wipContentContains(needle: string): boolean {
  return wipFiles().some((f) => {
    try {
      return fs.readFileSync(path.join(WIP_DIR, f), 'utf-8').includes(needle)
    } catch {
      return false
    }
  })
}

test('gateway 硬杀中断 → 重启 →「继续」→ 中断现场注入真实 LLM 请求（wip 侧车验收）', async ({ browser }) => {
  test.setTimeout(300_000)
  const page = await newAuthedPage(browser)
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })

  // ---- UI 建会话（种子纪律：必须经页面 UI 创建；断言 sid 与 locator 同源）----
  await openSessionSidebar(page)
  const prevActive = await page
    .locator('.session-item.active')
    .getAttribute('data-session-id')
    .catch(() => null)
  await page.locator('.fn-new-chat').click()
  await expect(async () => {
    const row = page.locator('.session-item.active')
    await expect(row.first()).toBeVisible({ timeout: 2_000 })
    const id = await row.first().getAttribute('data-session-id', { timeout: 1_000 })
    expect(id, `active id 未变：${prevActive}`).not.toBe(prevActive)
  }).toPass({ timeout: 15_000 })
  const sid = await page.locator('.session-item.active').getAttribute('data-session-id')
  expect(sid, '侧栏选中行应有 data-session-id').toBeTruthy()

  // ---- turn 1：慢速工具臂（mock 延迟 8s → sleep SLEEP_SECS 秒）----
  const evidenceBefore = evidenceCount()
  await sendChat(page, `E2E_SLOW_TOOL wip 中断现场验收 ${Date.now()}`)

  // 等 sleep 的 tool_start 落盘（tool_start 事件在 dispatch 点写入，先于执行）
  await expect(async () => {
    expect(wipContentContains('"tool":"sleep"'), 'wip 文件应出现 sleep 的 tool_start 事件').toBe(true)
  }).toPass({ timeout: 30_000 })

  // 负对照：kill 前到达 mock 的所有请求都无 wip 注入（含 turn 1 自己的请求
  // ——现场是本 turn 正在写的，无遗留可注入；宽松量化防杂务通道请求串位）
  const linesBeforeKill = evidenceLines().slice(evidenceBefore)
  expect(linesBeforeKill.length, 'turn 1 应有 mock 证据行').toBeGreaterThan(0)
  for (const l of linesBeforeKill) {
    expect(l, `kill 前证据行应无 wip 注入：${l}`).toContain('wip_scene=false')
  }

  // ---- 硬杀（supervisor taskkill /F /T = 真崩溃形态）+ 重启 + 重选会话 ----
  await restartGateway()
  await page.reload()
  await expect(page.locator('.chat-input-area textarea')).toBeVisible({ timeout: 20_000 })
  await openSessionSidebar(page)
  const target = page.locator(`.session-item[data-session-id="${sid}"]`)
  await expect(target).toBeVisible({ timeout: 20_000 })
  await target.click()

  // ---- turn 2：「继续」→ 消费遗留现场 → 注入 → mock 直答 → fold ----
  await sendChat(page, '继续')
  await waitAssistantReply(page, 'E2E_MOCK_TEXT_REPLY', 45_000)

  // 注入到达断言：证据行 + 完整请求体双面
  const resumedLines = evidenceLines().slice(evidenceBefore).filter((l) => l.includes('wip_scene=true'))
  expect(resumedLines.length, '「继续」的 LLM 请求应携带 wip 注入节（wip_scene=true）').toBeGreaterThan(0)
  const lastRequest = fs.readFileSync(EVIDENCE_LAST_REQUEST, 'utf-8')
  expect(lastRequest, '完整请求应含中断现场节标题').toContain('Interrupted Turn Scene')
  expect(lastRequest, '现场节应含被杀 turn 的工具事件渲染行').toContain('调用工具 sleep')
  // dump 是 JSON.stringify 形态——消息字符串里的内嵌 JSON 引号被转义，
  // 裸 '"seconds":12' 不命中；parse 后取消息字符串原文判断。
  const dump = JSON.parse(lastRequest) as { messages?: Array<{ content?: unknown }> }
  const joined = (dump.messages ?? [])
    .map((m) => (typeof m.content === 'string' ? m.content : ''))
    .join('\n')
  expect(
    joined,
    `现场节应含被杀 turn 的 sleep 参数 seconds=${SLEEP_SECS}（该 turn 未收尾，assistant 行未落盘，此参数唯一来源 = wip 注入）`,
  ).toContain(`"seconds":${SLEEP_SECS}`)

  // 折叠收口：turn 2 正常收尾后 wip 目录清空（fold：摘条目+删文件+清 pending）
  await expect(async () => {
    expect(wipFiles(), 'turn 收尾后 wip 目录应清空（fold）').toHaveLength(0)
  }).toPass({ timeout: 20_000 })

  await page.context().close()
})
