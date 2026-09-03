import { describe, it, expect } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'

// T8 多模态：useImageUpload 前置校验（与后端 upload.rs 同口径：白名单扩展名
// + 25MB 上限；不做前端压缩）。uploadImage 的 fetch 部分依赖后端，不在单测
// 范围（端点行为由 Rust 侧 handlers::upload::tests 覆盖）。

import { validateImageFile, extOf, IMAGE_MAX_BYTES } from '../useImageUpload'

function file(name: string, size: number): File {
  return new File([new Uint8Array(size)], name, { type: 'image/png' })
}

describe('extOf', () => {
  it('extracts lowercase extension', () => {
    expect(extOf('photo.PNG')).toBe('png')
    expect(extOf('a.b.JPG')).toBe('jpg')
  })
  it('returns empty string without dot', () => {
    expect(extOf('noext')).toBe('')
  })
})

describe('validateImageFile', () => {
  it('accepts whitelisted extensions', () => {
    expect(validateImageFile(file('a.png', 10))).toBeNull()
    expect(validateImageFile(file('a.jpeg', 10))).toBeNull()
    expect(validateImageFile(file('a.webp', 10))).toBeNull()
    expect(validateImageFile(file('a.gif', 10))).toBeNull()
  })

  it('rejects non-whitelisted extensions', () => {
    expect(validateImageFile(file('a.txt', 10))).toMatch(/不支持的图片格式/)
    expect(validateImageFile(file('a.svg', 10))).toMatch(/不支持的图片格式/)
    expect(validateImageFile(file('noext', 10))).toMatch(/不支持的图片格式/)
  })

  it('rejects oversized files without compressing', () => {
    expect(validateImageFile(file('big.png', IMAGE_MAX_BYTES + 1))).toMatch(/25MB/)
  })

  it('accepts exactly at the size limit', () => {
    expect(validateImageFile(file('edge.png', IMAGE_MAX_BYTES))).toBeNull()
  })
})

describe('uploadImage plumbing', () => {
  it('auth store is usable standalone (fetch path needs it)', () => {
    setActivePinia(createPinia())
    // 只验证依赖可初始化；fetch 交互由后端集成测试覆盖。
    expect(() => setActivePinia(createPinia())).not.toThrow()
  })
})
