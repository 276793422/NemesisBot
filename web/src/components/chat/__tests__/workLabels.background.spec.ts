import { describe, expect, it } from 'vitest'
import { jobIdFromPreview, toolLabel } from '../workLabels'

describe('toolLabel background 四条（2026-11-06 后台进程实时输出）', () => {
  it('映射到中文短标签', () => {
    expect(toolLabel('background_start')).toBe('启动后台进程')
    expect(toolLabel('background_output')).toBe('读后台输出')
    expect(toolLabel('background_kill')).toBe('停止后台进程')
    expect(toolLabel('background_steer')).toBe('注入后台输入')
  })
})

describe('jobIdFromPreview', () => {
  it('完整 JSON：取 job_id 数字字段', () => {
    expect(jobIdFromPreview('{"job_id":7,"running":true}')).toBe(7)
  })

  it('job_id 为 0 也是合法值（registry next_id 从 0 起）', () => {
    expect(jobIdFromPreview('{"job_id":0}')).toBe(0)
  })

  it('截断 JSON：正则抠 job_id 前缀', () => {
    const truncated = '{"job_id":42,"command":"ping -n 60 127.0.0.1","cwd":"C:\\\\work'
    expect(jobIdFromPreview(truncated)).toBe(42)
  })

  it('非数字 job_id：null（诚实降级为无 tail）', () => {
    expect(jobIdFromPreview('{"job_id":"7"}')).toBeNull()
    expect(jobIdFromPreview('{"running":true}')).toBeNull()
  })

  it('空/null/undefined 入参：null', () => {
    expect(jobIdFromPreview('')).toBeNull()
    expect(jobIdFromPreview(null)).toBeNull()
    expect(jobIdFromPreview(undefined)).toBeNull()
  })

  it('完全非 JSON 文本：null', () => {
    expect(jobIdFromPreview('error: boom')).toBeNull()
  })
})
