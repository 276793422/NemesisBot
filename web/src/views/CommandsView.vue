<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'

// 命令管理页（2026-08-29）：自定义 slash 命令的 CRUD。
// 四维度 = 命令名称 / 命令描述 / 参数提示 / 命令提示词（$ARGUMENTS 占位）。
// 命令本质是快捷提示词发送器：用户输入 /name args，AgentLoop 入口把模板
// 展开（$ARGUMENTS 替换为 args）后作为用户消息进入正常 LLM 轮次。
// 单一真相源 = config.commands.json；两个 TAB 都是它的视图，切换即从磁盘
// 刷新（最后写入者胜，同 HOOK 页模式）；保存走 commands.save（后端校验）。

interface CommandEntry {
  name: string
  description: string
  argument_hint: string
  prompt: string
}

const { request } = useWSAPI()
const toast = useToast()

const activeTab = ref<'overview' | 'editor'>('overview')
const commands = ref<CommandEntry[]>([])
const loading = ref(true)
const saving = ref(false)

const totalCount = computed(() => commands.value.length)

async function loadCommands() {
  try {
    const data = await request('commands', 'list')
    // 拷贝：不持有调用方数组引用（表单 push 不应污染数据源）。
    commands.value = [...(data?.commands || [])]
  } catch (e: any) {
    toast.error('加载命令失败: ' + e)
  }
  loading.value = false
}

function addCommand() {
  commands.value.push({ name: '', description: '', argument_hint: '', prompt: '' })
}

function removeCommand(i: number) {
  commands.value.splice(i, 1)
}

/** 前端轻校验（后端兜底同规则）：名称非空/无空格/唯一、提示词非空。 */
function validate(): string | null {
  const seen = new Set<string>()
  for (let i = 0; i < commands.value.length; i++) {
    const c = commands.value[i]
    const at = `第 ${i + 1} 条`
    const name = c.name.trim()
    if (!name) return `${at}：命令名称不能为空`
    if (/\s/.test(name)) return `${at}：命令名称不能包含空格（${name}）`
    if (seen.has(name)) return `${at}：命令名称重复（/${name}）`
    seen.add(name)
    if (!c.prompt.trim()) return `${at}（/${name}）：命令提示词不能为空`
  }
  return null
}

async function saveAll() {
  const err = validate()
  if (err) {
    toast.error(err)
    return
  }
  saving.value = true
  try {
    // 归一化：名称/描述/参数提示去首尾空白。
    const payload = commands.value.map(c => ({
      name: c.name.trim(),
      description: c.description.trim(),
      argument_hint: c.argument_hint.trim(),
      prompt: c.prompt,
    }))
    const data = await request('commands', 'save', { commands: payload })
    toast.success(`已保存（${data?.total ?? 0} 条命令）。命令表热更新，无需重启`)
    await loadCommands()
  } catch (e: any) {
    toast.error('保存被拒: ' + e)
  }
  saving.value = false
}

// 切换 TAB 即从磁盘刷新（丢弃另一 TAB 未保存的本地修改——最后写入者胜）。
watch(activeTab, () => {
  void loadCommands()
})

onMounted(loadCommands)
</script>

<template>
  <div class="page-commands">
    <div class="page-header"><h2>命令</h2></div>
    <div class="page-body">
      <div class="tabs">
        <button class="tab" :class="{ active: activeTab === 'overview' }" @click="activeTab = 'overview'">概览</button>
        <button class="tab" :class="{ active: activeTab === 'editor' }" @click="activeTab = 'editor'">命令管理</button>
      </div>

      <!-- ===================== 概览 ===================== -->
      <div v-if="activeTab === 'overview'">
        <div class="card">
          <div class="card-header"><h3>命令说明</h3></div>
          <div class="card-body">
            <p style="font-size: var(--text-sm); margin: 0; color: var(--text-secondary);">
              命令是<b>快捷提示词发送器</b>：在聊天输入框输入 <code>/命令名 参数</code>，Agent
              会把「命令提示词」模板（<code>$ARGUMENTS</code> 替换为参数）作为你的消息发给 LLM——
              类似 Claude Code 的 <code>/命令</code>。对所有通道生效；命令表保存后热更新，无需重启。
              内置命令（/help /model /show /list /switch）优先于同名自定义命令。
            </p>
          </div>
        </div>

        <div class="card" style="margin-top: var(--space-4);">
          <div class="card-header" style="justify-content: space-between;">
            <h3>已配置命令<span v-if="totalCount" style="font-weight: 400; font-size: var(--text-sm); color: var(--text-muted);">　共 {{ totalCount }} 条</span></h3>
            <button class="btn btn-sm" @click="loadCommands">重载</button>
          </div>
          <div class="card-body">
            <div v-if="!totalCount" class="empty-state" style="padding: var(--space-6); text-align: center; color: var(--text-muted);">
              暂无自定义命令——切到「命令管理」TAB 添加。
            </div>
            <template v-else>
              <div v-for="c in commands" :key="c.name" class="cmd-detail-row">
                <div style="display: flex; gap: var(--space-2); align-items: baseline;">
                  <span class="cmd-detail-name">/{{ c.name }}</span>
                  <span style="color: var(--text-secondary); font-size: var(--text-sm);">{{ c.description }}</span>
                  <span v-if="c.argument_hint" class="cmd-detail-hint">{{ c.argument_hint }}</span>
                </div>
                <pre class="cmd-detail-prompt">{{ c.prompt }}</pre>
              </div>
            </template>
          </div>
        </div>
      </div>

      <!-- ===================== 命令管理（结构化编辑） ===================== -->
      <div v-else>
        <div class="card" style="margin-bottom: var(--space-4);">
          <div class="card-header" style="justify-content: space-between;">
            <h3 style="margin: 0;">命令列表</h3>
            <div style="display: flex; gap: var(--space-2);">
              <button class="btn btn-sm" @click="loadCommands" :disabled="loading || saving">重载</button>
              <button class="btn btn-sm" @click="addCommand" :disabled="saving">+ 添加命令</button>
              <button class="btn btn-sm btn-primary" @click="saveAll" :disabled="loading || saving">
                {{ saving ? '保存中…' : '保存全部' }}
              </button>
            </div>
          </div>
          <div class="card-body">
            <p style="font-size: var(--text-xs); color: var(--text-muted); margin: 0 0 var(--space-2);">
              保存走后端语义校验（名称非空/无空格/唯一、提示词非空），校验失败不落盘；保存后热更新，无需重启。
            </p>
            <div v-if="!commands.length" class="empty-state" style="padding: var(--space-6); text-align: center; color: var(--text-muted);">
              暂无自定义命令——点右上角「+ 添加命令」创建。
            </div>

            <div v-for="(c, i) in commands" :key="i" class="card" style="margin-bottom: var(--space-3);">
              <div class="card-header" style="justify-content: space-between;">
                <h3 style="margin: 0; font-family: var(--font-mono);">
                  /{{ c.name || '?' }}
                  <span v-if="c.description" style="font-weight: 400; font-size: var(--text-sm); color: var(--text-muted);">{{ c.description }}</span>
                </h3>
                <button class="btn btn-sm" style="color: var(--danger);" @click="removeCommand(i)">删除</button>
              </div>
              <div class="card-body">
                <div style="display: flex; gap: var(--space-3); margin-bottom: var(--space-2);">
                  <div class="form-group" style="flex: 1;">
                    <label class="form-label">命令名称</label>
                    <input class="form-input" style="font-family: var(--font-mono);" v-model="c.name" placeholder="如 review（不带 /）" />
                  </div>
                  <div class="form-group" style="flex: 2;">
                    <label class="form-label">命令描述</label>
                    <input class="form-input" v-model="c.description" placeholder="补全菜单里展示的一句话说明" />
                  </div>
                  <div class="form-group" style="flex: 1;">
                    <label class="form-label">参数提示</label>
                    <input class="form-input" v-model="c.argument_hint" placeholder="如 &lt;文件路径&gt;（可空）" />
                  </div>
                </div>
                <div class="form-group">
                  <label class="form-label">命令提示词（<code>$ARGUMENTS</code> = 命令后追加的参数；未写占位符时参数会追加在末尾）</label>
                  <textarea class="form-textarea" style="min-height: 80px; font-family: var(--font-mono); font-size: var(--text-xs);" v-model="c.prompt" placeholder="如：请审查 $ARGUMENTS 的代码质量，重点关注安全与性能问题"></textarea>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 概览只读明细行 */
.cmd-detail-row {
  padding: var(--space-2) 0;
  border-bottom: 1px dashed var(--border-light);
}
.cmd-detail-row:last-child { border-bottom: none; }
.cmd-detail-name {
  font-family: var(--font-mono);
  font-weight: 600;
  color: var(--text-primary);
}
.cmd-detail-hint {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  padding: 1px 6px;
  border: 1px solid var(--border-light);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-muted);
}
.cmd-detail-prompt {
  margin: var(--space-1) 0 0;
  padding: var(--space-2) var(--space-3);
  background: var(--bg-secondary);
  border: 1px solid var(--border-light);
  border-radius: var(--radius-sm);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  white-space: pre-wrap;
  word-break: break-all;
  color: var(--text-secondary);
}
</style>
