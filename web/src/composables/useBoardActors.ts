import { ref } from 'vue'
import { useWSAPI } from './useWSAPI'

// H1（goal P1）：设备名可读化的单一解析层。
// 看板全链的 actor 标识是节点运行时 id（UUID 串），此前评论流/讨论组/
// 决策流/收件箱四处裸渲染。本 composable 拉一次 nodes.list 建立映射，
// 供所有界面显示「agent/Alex」这类可读名；未知 id 回退短 id + title 悬浮。
// 数据仅读不改（id 是稳定主键，name 是显示属性——单一真相源在集群注册表）。

export interface BoardActorNode {
  id: string
  name: string
  role: string
  category: string
  online: boolean
}

// 模块级共享缓存：全应用一份（各面板同源，不重复拉取）。
const nodes = ref<Map<string, BoardActorNode>>(new Map())
let loaded = false
let loading: Promise<void> | null = null

/** 短 id（未知节点的回退显示）。 */
export function shortId(id: string): string {
  return id.length > 14 ? `${id.slice(0, 12)}…` : id
}

/** 纯函数：在给定映射下解析可读名（便于单测）。
 *  未知 id：短形态（≤20 字符，如 "board"/"node-b"）原样保留——它们本身
 *  可读；UUID 长串（>20 且带连字符）→ 未知节点(短id)。 */
export function displayActorIn(
  map: Map<string, BoardActorNode>,
  kind: string,
  id: string,
): string {
  const name = map.get(id)?.name
  if (name) return `${kind}/${name}`
  if (id.length <= 20) return `${kind}/${id}`
  return `${kind}/未知节点(${shortId(id)})`
}

/** 纯函数：定向场景双显「Alex (node-node-8c45…)」——需要精确指认时用。 */
export function actorDualIn(map: Map<string, BoardActorNode>, id: string): string {
  const name = map.get(id)?.name
  return name ? `${name} (${shortId(id)})` : id
}

export function useBoardActors() {
  const { request } = useWSAPI()

  /** 拉取/刷新节点注册表（幂等；失败可重试）。 */
  async function ensureNodes(): Promise<void> {
    if (loaded) return
    if (!loading) {
      loading = request('cluster', 'nodes.list', {})
        .then((r) => {
          const map = new Map<string, BoardActorNode>()
          for (const n of r?.nodes || []) {
            if (n?.id) map.set(n.id, n as BoardActorNode)
          }
          nodes.value = map
          loaded = true
        })
        .catch(() => {
          loading = null // 拉取失败清 loading，下次 ensure 重试
        })
    }
    await loading
  }

  /** 可读名（未知 id → null，由调用方决定回退形态）。 */
  function actorName(id: string): string | null {
    return nodes.value.get(id)?.name ?? null
  }

  /** H1：kind + 可读名（`agent/Alex`）；未知 id → `agent/<短id>`。 */
  function displayActor(kind: string, id: string): string {
    return displayActorIn(nodes.value, kind, id)
  }

  /** H4：定向双显「Alex (node-node-8c45…)」；未知 id 原样。 */
  function actorDual(id: string): string {
    return actorDualIn(nodes.value, id)
  }

  return { nodes, ensureNodes, actorName, displayActor, actorDual }
}
