/**
 * 测试环境兜底（BUG 2026-09-22）：Node ≥26 无条件声明 localStorage/
 * sessionStorage 全局（惰性 getter，未给 `--localstorage-file` 时返回
 * undefined）。vitest 2 的 jsdom 环境拷贝 window 属性时对宿主已有的 key
 * 走白名单过滤（getWindowKeys 无小写 localStorage），jsdom 自己的存储
 * 实现被跳过，且 Node 的惰性 getter 同时污染 window 对象 → 测试全局的
 * localStorage 恒为 undefined，凡触碰即整文件崩（本机 Node 26 曾 121 个
 * 存量失败；CI 钉 Node 22 无此全局故从未红过）。
 *
 * 这里在 globalThis 上覆盖为内存版 Storage 实现：测试内读写、组件
 * setup 读写（仓库业务代码 21 处均为裸 `localStorage` 访问）命中同一
 * 份存储，测试环境与宿主 Node 版本彻底解耦。
 */
function installStorageShim(name: 'localStorage' | 'sessionStorage'): void {
  const g = globalThis as unknown as Record<string, unknown>;
  if (g[name] != null) {
    return; // 宿主已提供可用实现（如 jsdom 正常接管时）——不动
  }
  const area = new Map<string, string>();
  const shim: Storage = {
    get length(): number {
      return area.size;
    },
    clear(): void {
      area.clear();
    },
    getItem(key: string): string | null {
      return area.has(key) ? area.get(key)! : null;
    },
    key(index: number): string | null {
      return Array.from(area.keys())[index] ?? null;
    },
    removeItem(key: string): void {
      area.delete(key);
    },
    setItem(key: string, value: string): void {
      // Storage 只存字符串；与浏览器语义对齐（隐式 String() 转换）。
      area.set(key, String(value));
    },
  };
  Object.defineProperty(g, name, {
    value: shim,
    configurable: true,
    writable: false,
  });
}

installStorageShim('localStorage');
installStorageShim('sessionStorage');
