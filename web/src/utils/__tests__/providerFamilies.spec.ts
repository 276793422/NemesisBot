// P10（能力扩展 WS5，2026-09-25）：provider 家族推断/分组纯函数测试
// + 前端镜像表完整性（与 Rust PROVIDER_PRESETS 对齐的防漂移锁）。
// Rust 侧表数据完整性的权威测试在
// crates/nemesis-config/src/provider_resolver/tests.rs（唯一真相源）；
// 这里只保证前端镜像自身一致、分组语义正确。
import { describe, it, expect } from 'vitest'
import {
  PROVIDER_FAMILIES,
  UNKNOWN_FAMILY_ID,
  inferProviderFamily,
  groupModelsByFamily,
} from '../providerFamilies'

describe('PROVIDER_FAMILIES 镜像完整性', () => {
  it('家族规模与 Rust 预设表对齐（70 家）', () => {
    expect(PROVIDER_FAMILIES).toHaveLength(70)
  })

  it('id / aliases 全局唯一且 displayName 非空', () => {
    const seen = new Set<string>()
    for (const f of PROVIDER_FAMILIES) {
      expect(f.id.length).toBeGreaterThan(0)
      expect(f.displayName.length).toBeGreaterThan(0)
      expect(seen.has(f.id)).toBe(false)
      seen.add(f.id)
      for (const a of f.aliases) {
        expect(a.length).toBeGreaterThan(0)
        expect(seen.has(a)).toBe(false)
        seen.add(a)
      }
    }
  })

  it('核心家族与别名在镜像中存在（防误删漂移）', () => {
    const byId = new Map(PROVIDER_FAMILIES.map((f) => [f.id, f]))
    expect(byId.get('zhipu')?.aliases).toContain('glm')
    expect(byId.get('moonshot')?.aliases).toContain('kimi')
    expect(byId.get('openai')?.aliases).toContain('gpt')
    expect(byId.get('anthropic')?.aliases).toContain('claude')
    expect(byId.has('ollama')).toBe(true)
    expect(byId.has('github_copilot')).toBe(true)
    expect(byId.has('shengsuanyun')).toBe(true)
  })
})

describe('inferProviderFamily', () => {
  it('vendor 前缀精确命中（id 或别名）', () => {
    expect(inferProviderFamily('zhipu/glm-4.7')?.id).toBe('zhipu')
    expect(inferProviderFamily('kimi/moonshot-v1-8k')?.id).toBe('moonshot')
    expect(inferProviderFamily('openai/gpt-4o')?.id).toBe('openai')
    expect(inferProviderFamily('anthropic/claude-sonnet-4')?.id).toBe('anthropic')
    expect(inferProviderFamily('deepseek/deepseek-chat')?.id).toBe('deepseek')
    expect(inferProviderFamily('openrouter/openai/gpt-4o')?.id).toBe('openrouter')
  })

  it('大小写不敏感 + 去空白友好', () => {
    expect(inferProviderFamily('Zhipu/GLM-4.7')?.id).toBe('zhipu')
    expect(inferProviderFamily('MOONSHOT/x')?.id).toBe('moonshot')
  })

  it('裸型号名走包含匹配（无 vendor 前缀）', () => {
    expect(inferProviderFamily('glm-4.7-flash')?.id).toBe('zhipu')
    expect(inferProviderFamily('kimi-latest')?.id).toBe('moonshot')
    expect(inferProviderFamily('qwen-max')?.id).toBe('dashscope')
  })

  it('带 vendor 段但家族未知 → null（不做子串乱猜）', () => {
    expect(inferProviderFamily('myopenai-proxy/some-model')).toBeNull()
    expect(inferProviderFamily('unknown-vendor/mystery')).toBeNull()
  })

  it('完全未识别的裸名 → null（归「其他」组）', () => {
    expect(inferProviderFamily('mystery-model-9000')).toBeNull()
    expect(inferProviderFamily('')).toBeNull()
  })
})

describe('groupModelsByFamily', () => {
  interface Row {
    model: string
    name: string
  }

  function rows(...models: string[]): Row[] {
    return models.map((m, i) => ({ model: m, name: `n${i}` }))
  }

  it('按家族分组且保持表顺序（前沿 → 中国 → …）', () => {
    const groups = groupModelsByFamily(rows('zhipu/glm-4.7', 'openai/gpt-4o', 'groq/x'), (r) => r.model)
    expect(groups.map((g) => g.id)).toEqual(['openai', 'zhipu', 'groq'])
    // openai 在表中先于 zhipu/groq（表顺序优先于条目出现顺序）。
    expect(groups[0].label).toBe('OpenAI')
    expect(groups[0].items).toHaveLength(1)
  })

  it('未知条目归「其他」组且置底', () => {
    const groups = groupModelsByFamily(rows('mystery-9000', 'deepseek/deepseek-chat'), (r) => r.model)
    expect(groups.map((g) => g.id)).toEqual(['deepseek', UNKNOWN_FAMILY_ID])
    expect(groups[1].label).toBe('其他')
    expect(groups[1].items[0].model).toBe('mystery-9000')
  })

  it('空家族不出现；同家族条目聚合且序稳定', () => {
    const groups = groupModelsByFamily(
      rows('deepseek/deepseek-chat', 'deepseek/deepseek-reasoner', 'ollama/llama3.3'),
      (r) => r.model,
    )
    expect(groups).toHaveLength(2)
    expect(groups[0].items.map((r) => r.model)).toEqual([
      'deepseek/deepseek-chat',
      'deepseek/deepseek-reasoner',
    ])
  })

  it('空输入 → 空分组', () => {
    expect(groupModelsByFamily([], () => '')).toEqual([])
  })
})
