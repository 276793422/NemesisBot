/**
 * 会话侧栏顶部功能区两行布局视觉自测（2026-11-10，UI 视觉自测纪律）。
 *
 * 改动：fn-row 单行 5 按钮（第 5 个溢出）→ 两行 7 按钮
 *（行1 定时/Skills/MCP/命令；行2 批量/导入/工作区，新增 MCP/命令入口）。
 * 截图人工看图（test-results/sidebar-two-row-visual/shots/）。
 */
import { test } from './fixtures'
import path from 'node:path'
import { newAuthedPage, gotoRoute } from './helpers'

const SHOT_DIR = path.join('test-results', 'sidebar-two-row-visual', 'shots')

test('侧栏顶部两行按钮布局截图', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })

  await gotoRoute(page, '/#/')
  // 会话侧栏默认收起（showSidebar=false）→ 点聊天工具栏开合按钮展开
  const toggle = page.locator('.toolbar-toggle').first()
  await toggle.waitFor({ state: 'visible' })
  await toggle.click()
  const fnRow = page.locator('.fn-row')
  await fnRow.first().waitFor({ state: 'visible' })
  // 7 按钮 2 行就位
  const btnTexts = await page.locator('.fn-btn').allTextContents()
  console.log('[fn-btns]', JSON.stringify(btnTexts))
  if (btnTexts.length !== 7) {
    throw new Error(`期望 7 个 fn-btn，实际 ${btnTexts.length}: ${btnTexts.join(' | ')}`)
  }
  // 不溢出断言：每行右边缘不超过侧栏容器右边缘
  const overflow = await page.evaluate(() => {
    const sidebar = document.querySelector('.session-sidebar')
    const rows = document.querySelectorAll('.fn-row')
    if (!sidebar || rows.length !== 2) return 'sidebar/rows missing'
    const sr = sidebar.getBoundingClientRect()
    const bad: string[] = []
    rows.forEach((r, i) => {
      const rr = r.getBoundingClientRect()
      if (rr.right > sr.right + 1) bad.push(`row${i} right ${rr.right} > sidebar ${sr.right}`)
      r.querySelectorAll('.fn-btn').forEach((b) => {
        const br = b.getBoundingClientRect()
        if (br.right > sr.right + 1) bad.push(`row${i} btn "${b.textContent}" right ${br.right}`)
      })
    })
    return bad.length ? bad.join('; ') : 'ok'
  })
  console.log('[overflow-check]', overflow)
  if (overflow !== 'ok') throw new Error('按钮溢出侧栏: ' + overflow)

  await page.locator('.session-sidebar').screenshot({ path: path.join(SHOT_DIR, 'sidebar-two-rows.png') })
  await page.screenshot({ path: path.join(SHOT_DIR, 'full-page.png') })
})
