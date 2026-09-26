// Provider 家族表（前端镜像，P10 能力扩展 WS5，2026-09-25）
//
// 单一真相源是 Rust 侧 `crates/nemesis-config/src/provider_resolver.rs`
// 的 PROVIDER_PRESETS（P9 预设表）；这里只镜像 id / aliases / displayName
// 三字段供模型管理页做家族分组与筛选。新增/改名家族须两处同步
// （Rust 表是权威，前端跟随）。

export interface ProviderFamily {
  /** 规范家族 id（与 Rust 表 id / `model add --provider` 取值一致）。 */
  id: string
  /** 等价拼写（vendor 前缀或型号名包含推断用）。 */
  aliases: string[]
  /** 展示名（分组头/筛选项）。 */
  displayName: string
}

// 顺序 = Rust 表分区顺序（前沿 → 中国 → 国际 → 聚合/GPU 云 → 本地 → 特例）。
export const PROVIDER_FAMILIES: ProviderFamily[] = [
  // ---- 前沿实验室 ----
  { id: 'openai', aliases: ['gpt'], displayName: 'OpenAI' },
  { id: 'anthropic', aliases: ['claude'], displayName: 'Anthropic' },
  { id: 'gemini', aliases: ['google'], displayName: 'Google Gemini' },
  { id: 'xai', aliases: ['grok'], displayName: 'xAI' },
  // ---- 中国厂商 ----
  { id: 'zhipu', aliases: ['glm', 'bigmodel'], displayName: '智谱 AI' },
  { id: 'zai', aliases: [], displayName: 'Z.ai（智谱国际）' },
  { id: 'deepseek', aliases: [], displayName: 'DeepSeek' },
  { id: 'moonshot', aliases: ['kimi'], displayName: 'Moonshot AI（月之暗面）' },
  { id: 'dashscope', aliases: ['qwen', 'bailian'], displayName: '阿里云百炼（通义千问）' },
  { id: 'doubao', aliases: ['ark', 'volcengine'], displayName: '火山方舟（豆包）' },
  { id: 'hunyuan', aliases: ['tencent'], displayName: '腾讯混元' },
  { id: 'minimax', aliases: ['minimaxi'], displayName: 'MiniMax' },
  { id: 'baichuan', aliases: [], displayName: '百川智能' },
  { id: 'stepfun', aliases: [], displayName: '阶跃星辰' },
  { id: 'yi', aliases: ['01ai', 'lingyi'], displayName: '零一万物（01.AI）' },
  { id: 'siliconflow', aliases: ['silicon'], displayName: '硅基流动' },
  { id: 'modelscope', aliases: [], displayName: '魔搭社区' },
  { id: 'sensenova', aliases: ['sensetime'], displayName: '商汤日日新' },
  { id: 'ai360', aliases: ['360', 'qihoo'], displayName: '360 智脑' },
  { id: 'spark', aliases: ['xfyun', 'iflytek'], displayName: '讯飞星火' },
  { id: 'baidu', aliases: ['qianfan', 'ernie'], displayName: '百度千帆' },
  { id: 'gitee_ai', aliases: ['gitee'], displayName: 'Gitee AI' },
  // ---- 国际厂商 ----
  { id: 'mistral', aliases: [], displayName: 'Mistral AI' },
  { id: 'codestral', aliases: [], displayName: 'Codestral（Mistral 代码端点）' },
  { id: 'cohere', aliases: [], displayName: 'Cohere' },
  { id: 'perplexity', aliases: ['pplx'], displayName: 'Perplexity' },
  { id: 'ai21', aliases: ['jamba'], displayName: 'AI21 Labs' },
  { id: 'writer', aliases: [], displayName: 'Writer' },
  { id: 'reka', aliases: [], displayName: 'Reka AI' },
  { id: 'upstage', aliases: ['solar'], displayName: 'Upstage' },
  { id: 'gigachat', aliases: [], displayName: 'GigaChat（Sber）' },
  { id: 'yandex', aliases: [], displayName: 'Yandex Cloud' },
  { id: 'sarvam', aliases: [], displayName: 'Sarvam AI' },
  { id: 'llama_api', aliases: ['meta'], displayName: 'Meta Llama API' },
  // ---- 聚合 / GPU 云 ----
  { id: 'openrouter', aliases: [], displayName: 'OpenRouter' },
  { id: 'groq', aliases: [], displayName: 'Groq' },
  { id: 'together', aliases: ['together_ai'], displayName: 'Together AI' },
  { id: 'fireworks', aliases: ['fireworks_ai'], displayName: 'Fireworks AI' },
  { id: 'cerebras', aliases: [], displayName: 'Cerebras' },
  { id: 'sambanova', aliases: [], displayName: 'SambaNova' },
  { id: 'nvidia', aliases: ['nim'], displayName: 'NVIDIA NIM' },
  { id: 'deepinfra', aliases: [], displayName: 'DeepInfra' },
  { id: 'novita', aliases: ['novita_ai'], displayName: 'Novita AI' },
  { id: 'hyperbolic', aliases: [], displayName: 'Hyperbolic' },
  { id: 'nebius', aliases: [], displayName: 'Nebius AI Studio' },
  { id: 'lambda', aliases: ['lambdalabs'], displayName: 'Lambda' },
  { id: 'friendliai', aliases: [], displayName: 'FriendliAI' },
  { id: 'baseten', aliases: [], displayName: 'Baseten' },
  { id: 'kluster', aliases: [], displayName: 'Kluster AI' },
  { id: 'ovhcloud', aliases: ['ovh'], displayName: 'OVHcloud AI Endpoints' },
  { id: 'scaleway', aliases: [], displayName: 'Scaleway' },
  { id: 'gmi', aliases: [], displayName: 'GMI Cloud' },
  { id: 'nscale', aliases: [], displayName: 'Nscale' },
  { id: 'replicate', aliases: [], displayName: 'Replicate' },
  { id: 'huggingface', aliases: ['hf'], displayName: 'Hugging Face Router' },
  { id: 'github_models', aliases: ['github', 'ghmodels'], displayName: 'GitHub Models' },
  { id: 'vercel', aliases: ['vercel_gateway'], displayName: 'Vercel AI Gateway' },
  { id: 'featherless', aliases: [], displayName: 'Featherless AI' },
  { id: 'ionet', aliases: [], displayName: 'IO.NET Intelligence' },
  { id: 'ppinfra', aliases: ['ppio'], displayName: 'PPIO 派欧云' },
  { id: 'byteplus', aliases: [], displayName: 'BytePlus Model Ark' },
  { id: 'aihubmix', aliases: ['hubmix'], displayName: 'AiHubMix' },
  // ---- 本地推理服务器 ----
  { id: 'ollama', aliases: [], displayName: 'Ollama（本机）' },
  { id: 'lmstudio', aliases: ['lm_studio'], displayName: 'LM Studio（本机）' },
  { id: 'vllm', aliases: [], displayName: 'vLLM（本机）' },
  { id: 'sglang', aliases: [], displayName: 'SGLang（本机）' },
  { id: 'llama_cpp', aliases: ['llamacpp'], displayName: 'llama.cpp（本机）' },
  { id: 'jan', aliases: [], displayName: 'Jan（本机）' },
  // ---- 特例（历史遗留入口）----
  { id: 'github_copilot', aliases: ['copilot'], displayName: 'GitHub Copilot（本地代理）' },
  { id: 'shengsuanyun', aliases: [], displayName: '声通云路由' },
]

/** 未知家族条目的兜底分组 id（置底展示）。 */
export const UNKNOWN_FAMILY_ID = ''

/** 家族匹配名索引：id + 全部别名，长名优先（避免 "360" 抢先吃掉长命中）。 */
const FAMILY_NAME_INDEX: Array<{ name: string; family: ProviderFamily }> = PROVIDER_FAMILIES.flatMap(
  (f) => [f.id, ...f.aliases].map((name) => ({ name, family: f })),
).sort((a, b) => b.name.length - a.name.length)

/**
 * 从 model id（`vendor/model` 或裸名）推断 provider 家族。
 *
 * 规则：①`/` 前的 vendor 段精确匹配 id 或别名；②无 vendor 段或未命中时，
 * 按型号名包含匹配（最长名优先，≥2 字符才参与，降低误报）。未命中返回
 * null（调用方归入「其他」组）。
 */
export function inferProviderFamily(model: string): ProviderFamily | null {
  const lower = (model || '').toLowerCase()
  if (!lower) return null
  const vendor = lower.includes('/') ? lower.split('/')[0] : lower
  for (const { name, family } of FAMILY_NAME_INDEX) {
    if (name === vendor) return family
  }
  // 包含匹配只对无 vendor 前缀的裸名生效（有 vendor 段而未命中 = 未知家族，
  // 不做子串猜 family，避免 "myopenai-proxy/x" 误入 openai 组）。
  if (lower.includes('/')) return null
  for (const { name, family } of FAMILY_NAME_INDEX) {
    if (name.length >= 2 && lower.includes(name)) return family
  }
  return null
}

export interface ModelFamilyGroup<T> {
  /** 家族 id；''（UNKNOWN_FAMILY_ID）= 未识别的「其他」组（置底）。 */
  id: string
  /** 分组头展示名。 */
  label: string
  items: T[]
}

/**
 * 按家族分组，输出顺序 = 表顺序（与 Rust 预设表分区一致）；空家族省略；
 * 未识别条目进「其他」组置底。
 */
export function groupModelsByFamily<T>(items: T[], modelOf: (item: T) => string): Array<ModelFamilyGroup<T>> {
  const buckets = new Map<string, T[]>()
  const other: T[] = []
  for (const it of items) {
    const f = inferProviderFamily(modelOf(it))
    if (!f) {
      other.push(it)
      continue
    }
    let b = buckets.get(f.id)
    if (!b) {
      b = []
      buckets.set(f.id, b)
    }
    b.push(it)
  }
  const out: Array<ModelFamilyGroup<T>> = []
  for (const fam of PROVIDER_FAMILIES) {
    const b = buckets.get(fam.id)
    if (b && b.length > 0) out.push({ id: fam.id, label: fam.displayName, items: b })
  }
  if (other.length > 0) out.push({ id: UNKNOWN_FAMILY_ID, label: '其他', items: other })
  return out
}
