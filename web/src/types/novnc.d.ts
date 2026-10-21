// @novnc/novnc 无自带类型（ESM 组件库，exports 指向 core/rfb.js）。
// 这里只声明本仓消费面（RFB 类 + 构造选项）；完整 API 见
// node_modules/@novnc/novnc/core/rfb.js 与上游文档。
declare module '@novnc/novnc' {
  export interface RfbOptions {
    /** 共享模式（允许多 client 并连；QEMU VNC 恒接受）。 */
    shared?: boolean
    credentials?: { password?: string }
  }

  /** RFB 事件 detail 常用字段（noVNC 事件 detail 形态各异，按需取）。 */
  export interface RfbDisconnectDetail {
    clean: boolean
  }

  export default class RFB {
    constructor(target: HTMLElement, url: string, options?: RfbOptions)
    disconnect(): void
    focus(options?: { preventScroll?: boolean }): void
    blur(): void
    /** 桌面分辨率跟随 viewer 视口（SetDesktopSize；rw 语义）。 */
    scaleViewport: boolean
    resizeSession: boolean
    /** 只读观察：不发送键鼠/剪贴板输入。 */
    viewOnly: boolean
    qualityLevel: number
    compressionLevel: number
    background: string
    addEventListener(
      type: string,
      listener: (ev: { detail?: unknown }) => void,
      options?: boolean | AddEventListenerOptions,
    ): void
    removeEventListener(
      type: string,
      listener: (ev: { detail?: unknown }) => void,
      options?: boolean | EventListenerOptions,
    ): void
  }
}
