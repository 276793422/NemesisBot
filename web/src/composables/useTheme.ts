import { ref, watch } from 'vue'

export interface ColorScheme {
  id: string
  name: string
  accent: string
  accentHover: string
  accentMuted: string
}

export const PRESET_SCHEMES: ColorScheme[] = [
  {
    id: 'coral',
    name: '珊瑚',
    accent: '#E8705A',
    accentHover: '#F08A78',
    accentMuted: 'rgba(232, 112, 90, 0.12)',
  },
  {
    id: 'ocean',
    name: '海洋',
    accent: '#5A9AE8',
    accentHover: '#78B0F0',
    accentMuted: 'rgba(90, 154, 232, 0.12)',
  },
  {
    id: 'forest',
    name: '森林',
    accent: '#5AE878',
    accentHover: '#78F090',
    accentMuted: 'rgba(90, 232, 120, 0.12)',
  },
  {
    id: 'sunset',
    name: '日落',
    accent: '#E8A85A',
    accentHover: '#F0C078',
    accentMuted: 'rgba(232, 168, 90, 0.12)',
  },
  {
    id: 'lavender',
    name: '薰衣草',
    accent: '#A85AE8',
    accentHover: '#C078F0',
    accentMuted: 'rgba(168, 90, 232, 0.12)',
  },
  {
    id: 'rose',
    name: '玫瑰',
    accent: '#E85A8A',
    accentHover: '#F078A0',
    accentMuted: 'rgba(232, 90, 138, 0.12)',
  },
  {
    id: 'mint',
    name: '薄荷',
    accent: '#5AE8C8',
    accentHover: '#78F0D8',
    accentMuted: 'rgba(90, 232, 200, 0.12)',
  },
  {
    id: 'amber',
    name: '琥珀',
    accent: '#E8C85A',
    accentHover: '#F0D878',
    accentMuted: 'rgba(232, 200, 90, 0.12)',
  },
]

type Theme = 'dark' | 'light'

const theme = ref<Theme>((() => {
  const saved = localStorage.getItem('nemesisbot_theme')
  if (saved === 'light' || saved === 'dark') return saved
  if (window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches) {
    return 'light'
  }
  return 'dark'
})())

const colorScheme = ref<ColorScheme>((() => {
  const saved = localStorage.getItem('nemesisbot_color_scheme')
  if (saved) {
    try {
      const parsed = JSON.parse(saved)
      if (parsed && parsed.id && parsed.accent) return parsed
    } catch { /* ignore */ }
  }
  return PRESET_SCHEMES[0]
})())

function applyTheme(t: Theme) {
  document.documentElement.setAttribute('data-theme', t)
  localStorage.setItem('nemesisbot_theme', t)
}

function applyColorScheme(scheme: ColorScheme) {
  const root = document.documentElement
  root.style.setProperty('--accent', scheme.accent)
  root.style.setProperty('--accent-hover', scheme.accentHover)
  root.style.setProperty('--accent-muted', scheme.accentMuted)
  localStorage.setItem('nemesisbot_color_scheme', JSON.stringify(scheme))
}

// Apply on load
applyTheme(theme.value)
applyColorScheme(colorScheme.value)

watch(theme, (val) => {
  applyTheme(val)
})

watch(colorScheme, (val) => {
  applyColorScheme(val)
})

export function useTheme() {
  function toggleTheme() {
    theme.value = theme.value === 'dark' ? 'light' : 'dark'
  }

  function setTheme(t: Theme) {
    theme.value = t
  }

  function setColorScheme(scheme: ColorScheme) {
    colorScheme.value = scheme
  }

  function createCustomScheme(name: string, accent: string): ColorScheme {
    const r = parseInt(accent.slice(1, 3), 16)
    const g = parseInt(accent.slice(3, 5), 16)
    const b = parseInt(accent.slice(5, 7), 16)
    const hover = `rgb(${Math.min(255, r + 24)}, ${Math.min(255, g + 24)}, ${Math.min(255, b + 24)})`
    const muted = `rgba(${r}, ${g}, ${b}, 0.12)`
    return {
      id: `custom-${Date.now()}`,
      name,
      accent,
      accentHover: hover,
      accentMuted: muted,
    }
  }

  return {
    theme,
    colorScheme,
    toggleTheme,
    setTheme,
    setColorScheme,
    createCustomScheme,
    presets: PRESET_SCHEMES,
  }
}
