/// <reference types="vite/client" />

declare module '*.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<{}, {}, any>
  export default component
}

interface Window {
  __DASHBOARD_TOKEN__?: string
  __DASHBOARD_BACKEND__?: string
  runtime?: {
    EventsOn(event: string, callback: (...args: any[]) => void): void
  }
}
