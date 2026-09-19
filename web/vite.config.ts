import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { resolve } from 'path'

export default defineConfig({
  root: '.',
  // 桥子路径相对构建（goal：反向桥与多设备汇聚，一期批次二）：产物资源
  // 引用用相对路径（`./assets/x.js`），配合设备侧注入的 `<base href>`，
  // 直连（base=/）与经桥（base=/d/<node_id>/）都落到正确路径。路由为
  // hash 模式（createWebHashHistory），无 history 路径的相对解析坑。
  base: './',
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
        // L4 会话分享：独立只读页（token 即凭据，无登录态依赖）。
        share: resolve(__dirname, 'share/index.html'),
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
