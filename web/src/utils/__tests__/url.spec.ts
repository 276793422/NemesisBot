// safeExternalUrl 单测：外链 scheme 闸（BUG-20261101-274 渲染闸）。
// 契约：http(s) 绝对 URL 放行（大小写不敏感、前导空白容忍）；其余
// scheme（javascript:/data:/vbscript:）与相对串诚实返空串。
import { describe, expect, it } from 'vitest'
import { safeExternalUrl } from '../url'

describe('safeExternalUrl', () => {
  it('http/https 放行', () => {
    expect(safeExternalUrl('https://github.com/a/b/pull/1')).toBe(
      'https://github.com/a/b/pull/1',
    )
    expect(safeExternalUrl('http://localhost:8080/x')).toBe('http://localhost:8080/x')
  })

  it('scheme 大小写不敏感', () => {
    expect(safeExternalUrl('HTTPS://X/pr/1')).toBe('HTTPS://X/pr/1')
    expect(safeExternalUrl('Http://x/')).toBe('Http://x/')
  })

  it('前导空白容忍（模型爱加空格）', () => {
    expect(safeExternalUrl('  https://x/pr/2')).toBe('https://x/pr/2')
  })

  it('活动 scheme 丢弃', () => {
    expect(safeExternalUrl('javascript:alert(1)')).toBe('')
    expect(safeExternalUrl('JAVASCRIPT:x')).toBe('')
    expect(safeExternalUrl('data:text/html,<b>x</b>')).toBe('')
    expect(safeExternalUrl('vbscript:x')).toBe('')
    expect(safeExternalUrl(' javascript:alert(1)')).toBe('')
  })

  it('相对路径/裸串丢弃', () => {
    expect(safeExternalUrl('/pull/1')).toBe('')
    expect(safeExternalUrl('u9')).toBe('')
  })

  it('空值返空串', () => {
    expect(safeExternalUrl('')).toBe('')
    expect(safeExternalUrl(null)).toBe('')
    expect(safeExternalUrl(undefined)).toBe('')
  })
})
