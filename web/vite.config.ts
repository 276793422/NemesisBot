import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { resolve } from 'path'

const backendTarget = process.env.VITE_BACKEND || 'http://127.0.0.1:49000'

export default defineConfig({
  root: '.',
  plugins: [vue()],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src'),
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: backendTarget,
        changeOrigin: true,
      },
      '/ws': {
        target: backendTarget,
        changeOrigin: true,
        ws: true,
      },
    },
  },
  build: {
    outDir: '../crates/nemesis-web/static',
    emptyOutDir: false,
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
