<script setup lang="ts">
/**
 * P2-2 (2026-08-24 UI entry gap): 「二次开发」页 —— SDK 内嵌导出。
 *
 * 只拿 exe 的机器也能拿到完整 Python SDK：SDK 源码树在编译期打 zip
 * 嵌进 exe（build.rs，排除 build/egg-info 等产物），本页两个下载按钮
 * 直接吐字节（零磁盘 IO）。
 *  - /api/sdk/export —— SDK 目录 zip（解压即可浏览/改造源码）
 *  - /api/sdk/pip   —— sdist 布局 zip（pip install ./<file>.zip）
 */

import { ref } from 'vue'
import { useToast } from '../composables/useToast'
import { useAuthStore } from '../stores/auth'

const toast = useToast()
const auth = useAuthStore()

const downloading = ref<'export' | 'pip' | null>(null)

async function download(kind: 'export' | 'pip') {
  if (downloading.value) return
  downloading.value = kind
  try {
    const resp = await fetch(`/api/sdk/${kind}`, {
      headers: auth.token ? { 'X-Auth-Token': auth.token } : {},
    })
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`)
    const blob = await resp.blob()
    // 文件名取 Content-Disposition（后端带版本号）；取不到就用兜底名。
    const cd = resp.headers.get('Content-Disposition') || ''
    const m = cd.match(/filename="([^"]+)"/)
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = m?.[1] ?? `nemesisbot-sdk-${kind}.zip`
    document.body.appendChild(a)
    a.click()
    a.remove()
    URL.revokeObjectURL(url)
    toast.success('SDK 已开始下载')
  } catch (e: any) {
    toast.error('下载失败: ' + (e?.message || e))
  }
  downloading.value = null
}

const sampleCode = `from nemesisbot import NemesisBot

bot = NemesisBot(workspace="./mybot")
print(bot.turn("List the files in this directory."))`
</script>

<template>
  <div class="page-sdk">
    <div class="page-header"><h2>二次开发</h2></div>
    <div class="page-body">
      <div style="display: flex; flex-direction: column; gap: var(--space-4);">

        <!-- 说明卡 -->
        <div class="card">
          <div class="card-header"><h3>Python SDK 是什么</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <p class="muted">
              官方 Python SDK，把 NemesisBot 当作子进程驱动：自动拉起
              <code>nemesisbot --local gateway</code>、经 WebSocket 对话、
              用完自动收尾。适合把 bot 嵌进你自己的脚本 / 服务 / 自动化管线，
              不必自己管进程生命周期和 WS 协议。
            </p>
            <div>
              <div class="section-title">3 行示例（自动管理生命周期）</div>
              <pre class="code-block">{{ sampleCode }}</pre>
            </div>
            <p class="form-hint">
              首次在新工作区使用需先配模型（<code>nemesisbot model add --model &lt;vendor/model&gt; --key &lt;KEY&gt; --default</code>），与任何网关一致。
              多轮长驻用法（<code>with NemesisBot(...) as bot: bot.send(...)</code>）见包内 README。
            </p>
          </div>
        </div>

        <!-- 下载卡 -->
        <div class="card">
          <div class="card-header"><h3>导出</h3></div>
          <div style="padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3);">
            <div style="display: flex; gap: var(--space-3); flex-wrap: wrap;">
              <button class="btn" :disabled="downloading !== null" @click="download('export')">
                {{ downloading === 'export' ? '下载中…' : '导出 SDK 目录（zip）' }}
              </button>
              <button class="btn" :disabled="downloading !== null" @click="download('pip')">
                {{ downloading === 'pip' ? '下载中…' : '下载 pip 包（sdist zip）' }}
              </button>
            </div>
            <div style="display: flex; flex-direction: column; gap: var(--space-2); font-size: var(--text-sm);">
              <div>
                <strong>导出 SDK 目录</strong> —— zip 内即完整源码树（含 README / 示例 / 测试），
                解压后 <code>pip install .</code> 或直接改源码。
              </div>
              <div>
                <strong>下载 pip 包</strong> —— sdist 布局（单顶层目录），
                下载后直接 <code>pip install ./nemesisbot-sdk-pip-x.y.z.zip</code>，
                pip 会现场构建安装。
              </div>
            </div>
            <p class="form-hint">
              两个包内容同源（编译期从仓库 SDK 目录打包，已排除 build / egg-info 等构建产物）。
              需要联网拉取 <code>websockets</code> 依赖（pip 自动处理）。
            </p>
          </div>
        </div>

      </div>
    </div>
  </div>
</template>

<style scoped>
.section-title {
  font-size: var(--text-sm);
  font-weight: 600;
  margin-bottom: var(--space-2);
}
.code-block {
  background: var(--bg-inset, #1e1e1e);
  color: var(--text-primary, #d4d4d4);
  border-radius: var(--radius-md, 6px);
  padding: var(--space-3);
  font-family: Consolas, Monaco, 'Courier New', monospace;
  font-size: var(--text-sm);
  overflow-x: auto;
  margin: 0;
}
</style>
