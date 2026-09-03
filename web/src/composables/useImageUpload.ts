import { useAuthStore } from '../stores/auth'

/**
 * T8（多模态 goal 2026-09-03）：Dashboard 图片上传客户端。
 *
 * 协议：`POST /api/upload/image?name=<原始文件名>`，raw body（`fetch`
 * 直接把 File 当 body），鉴权 `X-Auth-Token` 同 Dashboard 约定。
 * 后端三道校验（扩展名白名单 / 25MB 上限 / magic byte）是真相源；
 * 这里的前置校验只为了即时 UX 反馈，不做 canvas 压缩（超限让用户自己
 * 缩小后重试，后端同样拒绝）。
 */

/** 与后端 upload.rs 白名单一致的前置清单。 */
export const IMAGE_EXT_WHITELIST = ['png', 'jpg', 'jpeg', 'webp', 'gif']

/** 与后端 MAX_IMAGE_BYTES 一致的上限（25MB）。 */
export const IMAGE_MAX_BYTES = 25 * 1024 * 1024

export interface UploadedImage {
  id: string
  path: string
  size: number
}

function backendBase(): string {
  // 与 useWebSocket.buildWSUrl 同源：desktop 内嵌窗口走注入的后端地址。
  const backend = (window as any).__DASHBOARD_BACKEND__
  if (backend) return 'http://' + backend
  return window.location.origin
}

export function extOf(name: string): string {
  const i = name.lastIndexOf('.')
  return i >= 0 ? name.slice(i + 1).toLowerCase() : ''
}

/** 前置校验：返回错误文案；null = 通过（后端仍会再验一遍）。 */
export function validateImageFile(file: File): string | null {
  if (!IMAGE_EXT_WHITELIST.includes(extOf(file.name))) {
    return `不支持的图片格式：${file.name}（仅支持 png/jpg/jpeg/webp/gif）`
  }
  if (file.size > IMAGE_MAX_BYTES) {
    return `图片超过 25MB 上限：${file.name}；不做前端压缩，请缩小后重试`
  }
  return null
}

/** 上传一张图片，成功返回后端给的 `{id, path, size}`（id 用于 chat.send media）。 */
export async function uploadImage(file: File): Promise<UploadedImage> {
  const invalid = validateImageFile(file)
  if (invalid) throw new Error(invalid)
  const auth = useAuthStore()
  const resp = await fetch(
    `${backendBase()}/api/upload/image?name=${encodeURIComponent(file.name)}`,
    { method: 'POST', headers: { 'X-Auth-Token': auth.token }, body: file },
  )
  if (!resp.ok) {
    let detail = `HTTP ${resp.status}`
    try {
      const j = await resp.json()
      if (j?.message) detail = j.message
      else if (j?.error) detail = j.error
    } catch {
      // 非 JSON 错误体，保留 HTTP 状态码文案
    }
    throw new Error(`图片上传失败：${detail}`)
  }
  return resp.json()
}
