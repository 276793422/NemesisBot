<template>
  <div class="card shop-card" @click="$emit('preview', p)">
    <div class="card-header">
      <span class="shop-card-emoji">{{ p.emoji }}</span>
      <h3>{{ p.name }}</h3>
    </div>
    <div class="card-body">
      <div class="shop-card-badges">
        <span class="badge badge-info">{{ p.division }}</span>
        <span v-if="p.source === 'builtin'" class="badge badge-neutral">内置</span>
        <!-- 推荐来源轻量角标（全部页展示；推荐橱窗行自身即是推荐语境，不重复 -->
        <span
          v-for="t in (showRecommendChips ? (p.recommended_for || []) : []).slice(0, 3)"
          :key="t"
          class="badge badge-neutral rec-chip"
          :title="'出现在推荐橱窗：' + t"
        >{{ t }}</span>
      </div>
      <p class="shop-card-desc">{{ p.description }}</p>
    </div>
    <div class="card-footer">
      <span v-if="p.installed" class="badge badge-neutral">已安装</span>
      <button
        v-else
        class="btn btn-sm btn-primary"
        @click.stop="$emit('download', p)"
        :disabled="busyDownload === p.id"
      >
        {{ busyDownload === p.id ? '下载中...' : '下载' }}
      </button>
      <button
        v-if="clusterEnabled"
        class="btn btn-sm"
        :class="{ 'btn-primary': hireEnabled }"
        :title="hireTitle"
        :disabled="!hireEnabled || busyHire === p.id"
        @click.stop="$emit('hire', p)"
      >
        {{ busyHire === p.id ? '聘用中...' : '集群聘用' }}
      </button>
      <button
        class="btn btn-sm"
        title="将此人格雇佣为本机专家子代理（在【专家Agent】页管理）；原文本地缺席时自动在线拉取保存"
        :disabled="busyHireExpert === p.id"
        @click.stop="$emit('hire-expert', p)"
      >
        {{ busyHireExpert === p.id ? '雇佣中...' : '雇佣专家' }}
      </button>
    </div>
  </div>
</template>

<script lang="ts">
// 人格超市共享卡片（2026-11-15 推荐首 TAB 改版抽出）：推荐橱窗行与全部页
// 渲染同一组件、消费同一份数据——状态（未下载/已安装）、下载、集群聘用、
// 雇佣专家两处完全同权（卡片同权原则）。共享类型从这里导出。
export interface ShopPersona {
  id: string
  name: string
  emoji: string
  division: string
  description: string
  installed: boolean
  /** 目录源 key（agency=上游仓库；builtin=编译期内嵌自建人格） */
  source: string
  /** 推荐橱窗行 title 列表（服务端策展表反查注入；未策展 = []） */
  recommended_for?: string[]
  raw?: string
  converted?: Record<string, string>
}

/// 聘用可用性（单一真相源：卡片与预览弹窗两处聘用按钮共用——2026-11-15
/// 八轮复查补弹窗臂）：builtin 支持免下载直聘用（后端 shop.hire builtin
/// 臂自动取材：live raw → 内嵌模板兜底，物化 personas/<id>/ 后写入集群）
/// ——未安装的内置卡可聘；agency 维持「先下载后聘用」（后端对未安装
/// agency 人格诚实拒绝，前端不按钮先行）。
export function isHireEnabled(p: ShopPersona): boolean {
  return p.installed || p.source === 'builtin'
}

export function hireTitleOf(p: ShopPersona): string {
  if (p.installed) return '将此人格写入集群人格文件并立即生效'
  if (p.source === 'builtin') return '免下载直聘：自动取材（在线优先，离线回落内嵌模板）'
  return '请先下载'
}
</script>

<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  p: ShopPersona
  busyDownload?: string
  busyHire?: string
  busyHireExpert?: string
  /** 全部页 true：展示推荐来源轻量角标；推荐橱窗行 false（不重复） */
  showRecommendChips?: boolean
}>()

defineEmits<{
  (e: 'preview', p: ShopPersona): void
  (e: 'download', p: ShopPersona): void
  (e: 'hire', p: ShopPersona): void
  (e: 'hire-expert', p: ShopPersona): void
}>()

// 集群聘用依赖 cluster WSAPI（裁剪构建可关）；雇佣专家不依赖 cluster 恒可用。
const clusterEnabled = import.meta.env.VITE_FEATURE_CLUSTER !== 'false'

// 聘用可用性判定走上方 isHireEnabled/hireTitleOf 单一真相源（卡片与
// 预览弹窗同源——弹窗臂 2026-11-15 八轮复查接线）。
const hireEnabled = computed(() => isHireEnabled(props.p))
const hireTitle = computed(() => hireTitleOf(props.p))
</script>

<style scoped>
.shop-card {
  cursor: pointer;
  transition: transform 0.15s, box-shadow 0.15s;
}

.shop-card:hover {
  transform: translateY(-2px);
  box-shadow: var(--shadow-md);
}

.shop-card-emoji {
  font-size: 1.5rem;
  margin-right: var(--space-2);
}

.shop-card .card-header {
  display: flex;
  align-items: center;
}

.shop-card .card-header h3 {
  margin: 0;
  font-size: var(--text-base);
}

.shop-card-badges {
  display: flex;
  gap: var(--space-1);
  flex-wrap: wrap;
  margin-bottom: var(--space-2);
}

.rec-chip {
  opacity: 0.8;
}

.shop-card-desc {
  margin: 0;
  color: var(--text-secondary);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  font-size: var(--text-sm);
}

.shop-card .card-footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  border-top: 1px solid var(--border);
}
</style>
