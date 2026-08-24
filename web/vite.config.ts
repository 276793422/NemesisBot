import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { resolve } from 'path'

export default defineConfig({
  root: '.',
  plugins: [vue()],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src'),
    },
  },
  build: {
    outDir: '../crates/nemesis-web/static',
    // outDir 在项目根之外（web/ → crates/nemesis-web/static/），Vite 默认不
    // 清空 → 陈旧 hash chunk 逐轮累积，全部被 include_dir! 嵌进 exe（2026-08-24
    // 复检实测累积 ~4MB/167 个文件、实际引用仅 ~33 个）。static/ 内容 100% 由
    // 本构建产出（三个 rollup 入口 + public/ 拷贝），可安全清空重建。
    emptyOutDir: true,
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'index.html'),
        chat: resolve(__dirname, 'chat/index.html'),
        workflowChat: resolve(__dirname, 'workflow-chat/index.html'),
      },
      output: {
        manualChunks: {
          'vendor-vue': ['vue', 'vue-router', 'pinia'],
          'vendor-echarts': ['echarts/core', 'echarts/charts', 'echarts/renderers', 'echarts/components', 'vue-echarts'],
        },
      },
    },
  },
})
