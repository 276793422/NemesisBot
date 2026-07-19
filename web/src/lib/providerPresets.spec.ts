import { describe, it, expect } from 'vitest'
import { PROVIDER_PRESETS, findProvider } from './providerPresets'
import { MCP_PRESETS } from './mcpPresets'
import { fieldMetaFor, CHANNEL_FIELD_META, SECURITY_FIELD_META } from './friendlyFields'

describe('wizard presets (no hand-typed vendor details)', () => {
  it('has multiple cloud providers with model lists', () => {
    expect(PROVIDER_PRESETS.length).toBeGreaterThanOrEqual(4)
    const openai = findProvider('openai')
    expect(openai?.models.length).toBeGreaterThan(0)
    expect(openai?.baseUrl).toContain('openai')
  })

  it('MCP presets cover common one-click servers', () => {
    const ids = MCP_PRESETS.map((p) => p.id)
    expect(ids).toContain('filesystem')
    expect(ids).toContain('github')
    expect(MCP_PRESETS.find((p) => p.id === 'filesystem')?.url).toBe('npx')
  })

  it('friendly field meta maps secrets to password inputs', () => {
    expect(fieldMetaFor('token', 'x', CHANNEL_FIELD_META).kind).toBe('password')
    expect(fieldMetaFor('enabled', true, CHANNEL_FIELD_META).kind).toBe('toggle')
    expect(SECURITY_FIELD_META.default_action.label).toContain('默认')
  })
})
