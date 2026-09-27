/**
 * 聊天委托桥（v3 chat 槽）：皮肤动作 → ChatPanel 真实发送/停止管线。
 *
 * 发送管线（校验、媒体、WSAPI chat.send 帧、会话锚定、停止语义）整体
 * 居住在 ChatPanel 组件内（1575 行 sendMessage / stopGeneration），皮肤
 * chat 槽**不复制**这段管线——ChatPanel 挂载时把真实入口注册进桥，皮肤
 * 动作（chat-send / chat-stop）经桥调用；卸载时注销（null = no-op，
 * 如路由不在聊天页时皮肤动作诚实空转）。
 *
 * 选委托而不是下沉到 store：3074 行组件内的管线牵扯会话锚定/媒体/在飞
 * 登记，复制一份必然漂移；SkinSlot 本就挂在 ChatPanel 内部，桥的生命
 * 周期与挂载天然同步。
 */

interface ChatBridgeHooks {
  send: (() => void) | null
  stop: (() => void) | null
}

const bridge: ChatBridgeHooks = { send: null, stop: null }

/** ChatPanel 挂载时注册真实管线入口（幂等覆盖）。 */
export function registerChatBridge(hooks: { send: () => void; stop: () => void }): void {
  bridge.send = hooks.send
  bridge.stop = hooks.stop
}

/** ChatPanel 卸载时注销。 */
export function unregisterChatBridge(): void {
  bridge.send = null
  bridge.stop = null
}

/** 皮肤动作入口（桥空 = no-op）。 */
export function bridgeSend(): void {
  bridge.send?.()
}

export function bridgeStop(): void {
  bridge.stop?.()
}
