import { ref, watch } from 'vue'

type Theme = 'dark' | 'light'

const theme = ref<Theme>((() => {
  const saved = localStorage.getItem('nemesisbot_theme')
  if (saved === 'light' || saved === 'dark') return saved
  if (window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches) {
    return 'light'
  }
  return 'dark'
})())

function applyTheme(t: Theme) {
  document.documentElement.setAttribute('data-theme', t)
  localStorage.setItem('nemesisbot_theme', t)
}

// Apply on load
applyTheme(theme.value)

watch(theme, (val) => {
  applyTheme(val)
})

export function useTheme() {
  function toggleTheme() {
    theme.value = theme.value === 'dark' ? 'light' : 'dark'
  }

  function setTheme(t: Theme) {
    theme.value = t
  }

  return { theme, toggleTheme, setTheme }
}
