<script setup lang="ts">
// 安全层页共享头（十层页面化 2026-11-10）：层号徽标 + 名称 + 状态徽标 +
// 职责说明条 + 生效时机 chip。SecurityLayerView 六层页与 ⑦⑨⑩ 三个独立
// 子系统页（Scanner/Sandbox/Vm）共用同一视觉入口（用户拍板 1：配置驱动
// 模板，不手写十页头）。
defineProps<{
  /** 层号（①…⑩；非十层页传空串隐藏徽标） */
  num?: string
  title: string
  /** 一句话职责说明（页头下方通栏说明条） */
  duty: string
  /** 状态徽标文案；空 = 不显示 */
  statusLabel?: string
  /** 徽标色调：on=绿 off=红 na=灰 warn=黄 */
  statusTone?: 'on' | 'off' | 'na' | 'warn'
  /** 生效时机：restart=重启网关生效 / live=即时生效；空 = 不显示 */
  effective?: 'restart' | 'live' | ''
}>()
</script>

<template>
  <div class="sec-layer-head">
    <div class="page-header">
      <h2>
        <span v-if="num" class="layer-num" data-test="layer-num">{{ num }}</span>
        {{ title }}
      </h2>
      <span
        v-if="statusLabel"
        class="layer-status"
        :class="'tone-' + (statusTone || 'na')"
        data-test="layer-status"
      >{{ statusLabel }}</span>
    </div>
    <div class="layer-duty" data-test="layer-duty">
      <svg class="duty-icon" viewBox="0 0 16 16" width="15" height="15" aria-hidden="true">
        <circle cx="8" cy="8" r="6.6" fill="none" stroke="currentColor" stroke-width="1.3" />
        <rect x="7.35" y="6.8" width="1.3" height="4.6" rx="0.65" fill="currentColor" />
        <circle cx="8" cy="4.6" r="0.9" fill="currentColor" />
      </svg>
      <span class="duty-text">{{ duty }}</span>
      <span v-if="effective === 'restart'" class="effective-chip">重启网关生效</span>
      <span v-else-if="effective === 'live'" class="effective-chip live">即时生效</span>
    </div>
  </div>
</template>

<style scoped>
.layer-num {
  display: inline-block;
  font-size: var(--text-md, 15px);
  background: var(--bg-elev, rgba(128, 128, 128, 0.12));
  border: 1px solid var(--border, rgba(128, 128, 128, 0.3));
  border-radius: var(--radius-sm, 6px);
  padding: 0 var(--space-2);
  margin-right: var(--space-1);
  vertical-align: middle;
}
.layer-status {
  font-size: var(--text-xs);
  padding: 2px 10px;
  border-radius: 999px;
  border: 1px solid currentColor;
}
.layer-status.tone-on {
  color: var(--success, #2da44e);
  background: color-mix(in srgb, var(--success, #2da44e) 12%, transparent);
}
.layer-status.tone-off {
  color: var(--error, #cf222e);
  background: color-mix(in srgb, var(--error, #cf222e) 12%, transparent);
}
.layer-status.tone-warn {
  color: var(--warning, #e5a00d);
  background: color-mix(in srgb, var(--warning, #e5a00d) 12%, transparent);
}
.layer-status.tone-na {
  color: var(--text-muted, #888);
  background: var(--bg-elev, rgba(128, 128, 128, 0.12));
}
.layer-duty {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
  font-size: var(--text-sm, 13px);
  line-height: 1.55;
  color: var(--text-secondary, #666);
  background: var(--bg-elev, rgba(128, 128, 128, 0.06));
  border: 1px solid var(--border, rgba(128, 128, 128, 0.22));
  border-radius: var(--radius-md, 8px);
  padding: var(--space-2) var(--space-3);
  /* 与 .page-body（padding: var(--space-6)）内的卡片左右对齐；上留 16px
     呼吸空隙（用户验收反馈：横贯带子 + 贴死页头难看，要正常卡片形态） */
  margin: var(--space-4) var(--space-6) 0;
}
.duty-icon {
  flex-shrink: 0;
  color: var(--primary, #4a89dc);
}
.duty-text {
  flex: 1;
  min-width: 200px;
}
.effective-chip {
  flex-shrink: 0;
  white-space: nowrap;
  padding: 1px 8px;
  border-radius: 999px;
  border: 1px dashed var(--warning, #e5a00d);
  color: var(--warning, #e5a00d);
}
.effective-chip.live {
  border-color: var(--success, #2da44e);
  color: var(--success, #2da44e);
}
</style>
