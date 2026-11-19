/**
 * 会话资源盘点看板布局视觉自测（2026-11-18，UI 视觉自测纪律）。
 *
 * 用户拍板布局（2026-11-18）：ChatPanel 恒在最左 + 固定宽 944，资源窗紧贴
 * 其右侧吃掉剩余宽度到窗口右缘。旧版缺陷形态：.chat-main-narrow 挂
 * margin-left:auto 把对话列推到中间（flex 剩余空间全落左侧）。
 *
 * 核心回归不变量 = 开合资源窗前后对话列左缘/位置不动（用户抱怨的正是
 * 「资源窗弹出后 chatpanel 跑中间去了」）。对话列左缘的外侧是应用主导航
 * 侧栏（AppLayout 层，约 240px），不属于本布局的断言范围。
 * 截图人工看图（test-results/resources-panel-visual/shots/）。
 */
import { test } from './fixtures'
import path from 'node:path'
import { newAuthedPage, gotoRoute } from './helpers'

const SHOT_DIR = path.join('test-results', 'resources-panel-visual', 'shots')

interface Geo {
  chatLeft: number
  chatWidth: number
  chatRight: number
  panelLeft: number
  panelWidth: number
  panelRight: number
  winWidth: number
}

function readChatRect(page: import('@playwright/test').Page): Promise<{ chatLeft: number; chatWidth: number; chatRight: number } | string> {
  return page.evaluate(() => {
    const chat = document.querySelector('.page-chat')?.getBoundingClientRect()
    if (!chat) return 'missing .page-chat'
    return { chatLeft: chat.left, chatWidth: chat.width, chatRight: chat.right }
  })
}

function readGeo(page: import('@playwright/test').Page): Promise<Geo | string> {
  return page.evaluate(() => {
    const chat = document.querySelector('.page-chat')?.getBoundingClientRect()
    const panel = document.querySelector('.chat-resources-panel')?.getBoundingClientRect()
    if (!chat || !panel) return 'missing .page-chat/.chat-resources-panel'
    return {
      chatLeft: chat.left,
      chatWidth: chat.width,
      chatRight: chat.right,
      panelLeft: panel.left,
      panelWidth: panel.width,
      panelRight: panel.right,
      winWidth: window.innerWidth,
    }
  })
}

test('开合不变量：资源窗展开后对话列原地不动、固定 944，资源窗紧贴右侧到窗口右缘', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1920, height: 1080 })
  // 先进收起态拿基线（对话列位置/宽度），再经 UI 打开——断言「打开不动对话列」
  await page.addInitScript(() => localStorage.setItem('nb_resources_panel', '0'))
  await gotoRoute(page, '/#/')

  await page.locator('.chat-resources-rail').waitFor({ state: 'visible' })
  const closed = await readChatRect(page)
  if (typeof closed === 'string') throw new Error(`收起态取几何失败: ${closed}`)
  console.log('[geo-closed-1920]', JSON.stringify(closed))

  await page.locator('.chat-resources-rail').click()
  const geo = await readGeo(page)
  console.log('[geo-open-1920]', JSON.stringify(geo))
  if (typeof geo === 'string') throw new Error(geo)
  const errors: string[] = []
  if (Math.abs(geo.chatWidth - 944) > 2) errors.push(`展开后对话列宽应为 944，实际 ${geo.chatWidth}`)
  if (Math.abs(geo.chatLeft - closed.chatLeft) > 2)
    errors.push(`展开后对话列左缘移动了（${closed.chatLeft} → ${geo.chatLeft}）——旧版缺陷形态`)
  if (Math.abs(geo.panelLeft - geo.chatRight) > 2)
    errors.push(`资源窗未紧贴对话列右缘（panel.left=${geo.panelLeft} vs chat.right=${geo.chatRight}）`)
  if (Math.abs(geo.panelRight - geo.winWidth) > 2)
    errors.push(`资源窗未伸到窗口右缘（panel.right=${geo.panelRight} vs win=${geo.winWidth}）`)
  if (errors.length) throw new Error(errors.join('; '))

  await page.screenshot({ path: path.join(SHOT_DIR, '1-open-1920.png') })
})

test('资源窗展开（1600 宽）：三件占满无死空间', async ({ browser }) => {
  // 注：不用贴近阈值的视口——RESOURCES_LAYOUT_MIN(1284) 量的是
  // chat-page-layout 容器（应用主导航侧栏 ~240px 在它外面），1400 视口容器
  // 只剩 ~1160 → 窄屏强收（按设计），面板永远不出现。1600-240=1360 ≥ 1284。
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1600, height: 900 })
  await gotoRoute(page, '/#/')

  await page.locator('.chat-resources-panel').waitFor({ state: 'visible' })
  const geo = await readGeo(page)
  console.log('[geo-open-1600]', JSON.stringify(geo))
  if (typeof geo === 'string') throw new Error(geo)
  const errors: string[] = []
  if (Math.abs(geo.chatWidth - 944) > 2) errors.push(`对话列宽应为 944，实际 ${geo.chatWidth}`)
  if (Math.abs(geo.panelLeft - geo.chatRight) > 2) errors.push('资源窗未紧贴对话列')
  if (Math.abs(geo.panelRight - geo.winWidth) > 2) errors.push('资源窗未到窗口右缘')
  if (geo.panelWidth < 300) errors.push(`资源窗宽 ${geo.panelWidth} < 最小 300`)
  if (errors.length) throw new Error(errors.join('; '))

  await page.screenshot({ path: path.join(SHOT_DIR, '2-open-1600.png') })
})

test('资源窗收起：对话列撑满到右缘细条，细条贴窗口右缘', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1920, height: 1080 })
  await page.addInitScript(() => localStorage.setItem('nb_resources_panel', '0'))
  await gotoRoute(page, '/#/')

  const rail = page.locator('.chat-resources-rail')
  await rail.waitFor({ state: 'visible' })
  const state = await page.evaluate(() => {
    const chat = document.querySelector('.page-chat')?.getBoundingClientRect()
    const rail = document.querySelector('.chat-resources-rail')?.getBoundingClientRect()
    if (!chat || !rail) return 'missing .page-chat/.chat-resources-rail'
    return { chatRight: chat.right, railLeft: rail.left, railRight: rail.right, winWidth: window.innerWidth }
  })
  console.log('[geo-closed-1920]', JSON.stringify(state))
  if (typeof state === 'string') throw new Error(state)
  const errors: string[] = []
  if (Math.abs(state.railLeft - state.chatRight) > 2) errors.push('细条未紧贴对话列右缘')
  if (Math.abs(state.railRight - state.winWidth) > 2) errors.push('细条未贴窗口右缘')
  if (errors.length) throw new Error(errors.join('; '))

  await page.screenshot({ path: path.join(SHOT_DIR, '3-closed-1920.png') })
})
