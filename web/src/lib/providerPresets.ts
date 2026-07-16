/**
 * Cloud LLM provider presets — users pick a provider + model + paste API key.
 * Base URL / model id filled automatically (no hand-typing vendor details).
 */

export interface ModelChoice {
  id: string
  label: string
}

export interface ProviderPreset {
  id: string
  label: string
  /** Default API base */
  baseUrl: string
  /** Suggested display name prefix */
  namePrefix: string
  models: ModelChoice[]
  /** Optional hint under the key field */
  keyHint?: string
}

export const PROVIDER_PRESETS: ProviderPreset[] = [
  {
    id: 'openai',
    label: 'OpenAI',
    baseUrl: 'https://api.openai.com/v1',
    namePrefix: 'OpenAI',
    keyHint: '以 sk- 开头的 API Key',
    models: [
      { id: 'gpt-4o', label: 'GPT-4o' },
      { id: 'gpt-4o-mini', label: 'GPT-4o mini' },
      { id: 'o3-mini', label: 'o3-mini' },
    ],
  },
  {
    id: 'zhipu',
    label: '智谱 (Zhipu)',
    baseUrl: 'https://open.bigmodel.cn/api/paas/v4',
    namePrefix: '智谱',
    models: [
      { id: 'zhipu/glm-4.7-flash', label: 'GLM-4.7 Flash' },
      { id: 'zhipu/glm-4-plus', label: 'GLM-4 Plus' },
      { id: 'zhipu/glm-4-flash', label: 'GLM-4 Flash' },
    ],
  },
  {
    id: 'deepseek',
    label: 'DeepSeek',
    baseUrl: 'https://api.deepseek.com/v1',
    namePrefix: 'DeepSeek',
    models: [
      { id: 'deepseek/deepseek-chat', label: 'DeepSeek Chat' },
      { id: 'deepseek/deepseek-reasoner', label: 'DeepSeek Reasoner' },
    ],
  },
  {
    id: 'anthropic',
    label: 'Anthropic',
    baseUrl: 'https://api.anthropic.com/v1',
    namePrefix: 'Anthropic',
    keyHint: 'Anthropic API Key',
    models: [
      { id: 'anthropic/claude-sonnet-4-20250514', label: 'Claude Sonnet 4' },
      { id: 'anthropic/claude-3-5-haiku-latest', label: 'Claude 3.5 Haiku' },
    ],
  },
  {
    id: 'moonshot',
    label: '月之暗面 (Kimi)',
    baseUrl: 'https://api.moonshot.cn/v1',
    namePrefix: 'Kimi',
    models: [
      { id: 'moonshot/moonshot-v1-auto', label: 'Moonshot Auto' },
      { id: 'moonshot/moonshot-v1-8k', label: 'Moonshot 8K' },
    ],
  },
  {
    id: 'custom',
    label: '其他 / 兼容 OpenAI 接口',
    baseUrl: '',
    namePrefix: '自定义',
    models: [],
    keyHint: '兼容 OpenAI 协议的服务',
  },
]

export function findProvider(id: string): ProviderPreset | undefined {
  return PROVIDER_PRESETS.find((p) => p.id === id)
}
