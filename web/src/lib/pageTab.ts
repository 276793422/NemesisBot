/**
 * Sync page-level tabs with `?tab=` query (hash router friendly).
 */
import { onMounted, watch, type Ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'

export function usePageTab(activeTab: Ref<string>, allowed: readonly string[], defaultTab?: string) {
  const route = useRoute()
  const router = useRouter()
  const fallback = defaultTab ?? allowed[0] ?? 'default'

  function applyFromRoute() {
    const q = route.query.tab
    const tab = typeof q === 'string' ? q : Array.isArray(q) ? q[0] : ''
    if (tab && allowed.includes(tab)) {
      activeTab.value = tab
    } else if (!allowed.includes(activeTab.value)) {
      activeTab.value = fallback
    }
  }

  function setTab(tab: string) {
    if (!allowed.includes(tab)) return
    activeTab.value = tab
    const nextQuery = { ...route.query, tab }
    // Avoid cluttering URL when on default tab
    if (tab === fallback) {
      const { tab: _drop, ...rest } = nextQuery as Record<string, unknown>
      router.replace({ query: rest as any }).catch(() => {})
    } else {
      router.replace({ query: nextQuery }).catch(() => {})
    }
  }

  onMounted(applyFromRoute)
  watch(() => route.query.tab, applyFromRoute)

  return { setTab, applyFromRoute }
}
