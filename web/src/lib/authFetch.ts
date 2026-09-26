import { useAuthStore } from '../stores/auth'
import { apiUrl } from './appBase'

/**
 * 带鉴权的 REST fetch（2026-09-22 统一鉴权配套）。
 *
 * 统一鉴权上线时只改了 useChatApi / useImageUpload / useSSE 三处，
 * OverviewView（httpGet）、EventStream、AboutView、LicenseView、
 * UsageView、RelayTab 的裸 fetch 全部漏改——auth_token 非空的部署
 * （Android Shell 等）REST 恒 401，页面数据全部停留在占位符。
 *
 * 语义对齐 useChatApi.apiFetch：X-Auth-Token 头来自 auth store；
 * 调用方自带的 headers 优先（可覆盖）。豁免路径（/api/sdk/ 等）
 * 带不带 token 都放行，统一走这里也无害。
 */
export function authedFetch(path: string, init?: RequestInit): Promise<Response> {
  const auth = useAuthStore()
  return fetch(apiUrl(path), {
    ...init,
    headers: {
      ...(auth.token ? { 'X-Auth-Token': auth.token } : {}),
      ...(init?.headers || {}),
    },
  })
}
