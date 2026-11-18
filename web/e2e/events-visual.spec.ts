/**
 * 事件规则表单编辑页视觉自测（2026-11-17，UI 视觉自测纪律；2026-11-18 双 TAB
 * 改版跟随——总览 = 信息展示，规则设置 = 表单编辑，用户拍板首页只留信息）。
 *
 * 事件规则从「直接编辑 JSON 脚本」改为钩子样式表单卡（用户拍板）。
 * 截图人工看图（test-results/events-visual/shots/）：
 *   1. /events 总览空态（说明卡 + 泵统计 + 空规则表 + cron/后台进程收编卡）
 *   1b. 规则设置 TAB 空态（新增按钮在场）
 *   2. 五族规则表单卡全览（fs/webhook/proc/sys/probe 动态字段组 + deliver 段）
 *   3. kind 切换到 probe 的探测字段组
 *   4. 会话 picker 弹层（单选形态；Teleport 到 body 不再被卡片裁剪）
 *   5. 校验错误态（必填缺失错误显示在保存钮旁）
 *
 * 规则数据经真实 WSAPI events.rules.save 灌入（不走文件注入——同源消费路径）。
 */
import { test, expect } from './fixtures'
import path from 'node:path'
import { newAuthedPage, gotoRoute, wsapi } from './helpers'

const SHOT_DIR = path.join('test-results', 'events-visual', 'shots')

function shotPath(name: string): string {
  return path.join(SHOT_DIR, name)
}

const FIVE_RULES = [
  {
    id: 'build-done',
    watch: { kind: 'fs', path: 'C:/logs/build', glob: '*.log', debounce_ms: 800, content_pattern: '(?i)fatal|panic' },
    deliver: { session_key: 'agent:main:session:seed-1', prompt: '构建日志有更新：{paths}', cooldown_secs: 60, merge_window_secs: 5, max_per_hour: 10, enabled: true },
  },
  {
    id: 'ci-hook',
    watch: { kind: 'webhook', token: 's3cret' },
    deliver: { session_key: 'agent:main:session:seed-1', prompt: '外部推送：{payload}', enabled: true },
  },
  {
    id: 'build-err',
    watch: { kind: 'proc', job_id: 7, pattern: '(?i)error|FAILED', notify_start: true },
    deliver: { session_key: 'agent:main:session:seed-1', prompt: '后台任务 #{job_id} 命中：{line}', enabled: true },
  },
  {
    id: 'cpu-hot',
    watch: { kind: 'sys', metric: 'cpu', above: 90.5, below: 80 },
    deliver: { session_key: 'agent:main:session:seed-2', prompt: '{payload}', enabled: true },
  },
  {
    id: 'api-down',
    watch: { kind: 'probe', probe_type: 'http', target: 'https://api.example.com/health', interval_secs: 30, timeout_secs: 5 },
    deliver: { session_key: 'agent:main:session:seed-2', prompt: '{payload}', enabled: true },
  },
]

test('事件规则表单页五景截图（空态/五族全览/probe组/会话picker/校验错误）', async ({ browser }) => {
  const page = await newAuthedPage(browser)
  await page.setViewportSize({ width: 1440, height: 900 })
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

  // —— 1. 空态总览（双 TAB 默认落总览：信息展示页）——
  await gotoRoute(page, '/#/events')
  await page.locator('[data-testid="events-tab-rules"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('1-events-empty.png'), fullPage: true })
  // —— 1b. 规则设置 TAB 空态（新增按钮在场）——
  await page.locator('[data-testid="events-tab-rules"]').click()
  await page.locator('[data-testid="add-rule-btn"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('1b-events-rules-empty.png'), fullPage: true })

  // —— 2. seed 五族规则（真实 WSAPI 保存路径）+ 重载看五张表单卡 ——
  // 注：gotoRoute 同 hash 不重挂载（status 投影停在挂载时旧数据），须 page.reload。
  // 高视口替代 fullPage：AppLayout 主区是内部滚动容器，文档高度=视口高度，
  // fullPage 截不到滚动容器内溢出内容。
  const saved = await wsapi(page, 'events', 'rules.save', { rules: FIVE_RULES })
  console.log('[seed] rules.save -> ' + JSON.stringify(saved).slice(0, 200))
  await page.reload()
  await page.locator('text=5 条').first().waitFor({ state: 'visible' })
  // 双 TAB：reload 落总览（规则表行数已可见），编辑区在第二页。
  await page.locator('[data-testid="events-tab-rules"]').click()
  const cards = page.locator('[data-testid^="rule-card-"]')
  await expect(cards).toHaveCount(5)
  await page.setViewportSize({ width: 1440, height: 3400 })
  await page.screenshot({ path: shotPath('2-events-five-families.png'), fullPage: true })

  // —— 3. kind 切换 fs → probe：探测字段组 ——
  await page.locator('[data-testid="rule-kind-0"]').selectOption('probe')
  await page.locator('text=探测形态').first().waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('3-events-probe-fields.png'), fullPage: true })
  // 切回 fs（后续景别按 fs 断言定位）。
  await page.locator('[data-testid="rule-kind-0"]').selectOption('fs')

  // —— 4. 会话 picker 弹层（单选形态）——
  await page.locator('[data-testid="pick-session-0"]').click()
  await page.locator('.ev-picker').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('4-events-session-picker.png') })
  await page.locator('.ev-picker-footer button', { hasText: '关闭' }).click()
  await page.locator('.ev-picker').waitFor({ state: 'hidden' })

  // —— 5. 校验错误态（清空 fs 必填 path → 错误显示在保存钮旁）——
  await page.locator('[data-testid="fs-path-0"]').fill('')
  await page.locator('[data-testid="save-rules-btn"]').click()
  await page.locator('[data-testid="rules-error"]').waitFor({ state: 'visible' })
  await page.screenshot({ path: shotPath('5-events-validation-error.png'), fullPage: true })

  if (consoleErrs.length > 0) {
    console.log('CONSOLE ERRORS:\n' + consoleErrs.join('\n'))
  }
})
