// Vitest 全局 setup（vitest.config.ts setupFiles）。
//
// Node 26 起注入实验性 localStorage 全局绑定（未加 --localstorage-file
// 标志时值恒为 undefined，见 ExperimentalWarning），该绑定会遮蔽 vitest
// jsdom 环境从 window 拷贝的 Storage 实现——凡挂载读 localStorage 的组件
// （TodoPanel、useSSE 等）在测试里全部 TypeError。旧 Node（CI）无此全局，
// 条件不成立、shim 不生效，行为与此前一致。
//
// shim 只需满足测试面对 Storage 的语义（get/set/remove/clear/length/key），
// 不需要真实持久化。
class MemoryStorage {
  private map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.has(k) ? this.map.get(k)! : null
  }
  setItem(k: string, v: string) {
    this.map.set(String(k), String(v))
  }
  removeItem(k: string) {
    this.map.delete(k)
  }
  clear() {
    this.map.clear()
  }
  key(i: number): string | null {
    return [...this.map.keys()][i] ?? null
  }
  get length(): number {
    return this.map.size
  }
}

if (typeof globalThis.localStorage === 'undefined' || (globalThis as any).localStorage == null) {
  Object.defineProperty(globalThis, 'localStorage', {
    value: new MemoryStorage(),
    configurable: true,
  })
}
if (typeof globalThis.sessionStorage === 'undefined' || (globalThis as any).sessionStorage == null) {
  Object.defineProperty(globalThis, 'sessionStorage', {
    value: new MemoryStorage(),
    configurable: true,
  })
}
