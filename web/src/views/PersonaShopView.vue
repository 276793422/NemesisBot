<template>
  <div class="page-persona-shop">
    <div class="page-header">
      <h2>人格超市</h2>
      <!-- 数据源徽标：在线 / 内置快照降级态 -->
      <span class="badge badge-neutral shop-source-badge" :class="{ 'shop-source-snapshot': dataSource === 'snapshot' }">
        {{ sourceLabel }}
      </span>
    </div>
    <div class="page-body">
      <!-- 超市内部 TAB（2026-11-15 橱窗改版）：推荐 = 进门第一眼（橱窗陈列），
           全部人格 = 细分查找往里走，内置人格 = 自建专列（独立页拍板——
           内置与 agency 不掺合，未来加自家专家人格只进此页）。默认落在推荐页。 -->
      <div class="tabs">
        <button
          class="tab"
          :class="{ active: activeTab === 'recommend' }"
          @click="switchTab('recommend')"
        >推荐</button>
        <button
          class="tab"
          :class="{ active: activeTab === 'all' }"
          @click="switchTab('all')"
        >全部人格</button>
        <button
          class="tab"
          :class="{ active: activeTab === 'builtin' }"
          @click="switchTab('builtin')"
        >内置人格</button>
      </div>

      <!-- Loading（挂载即在线刷新上游的窗口也挂转圈——shop.refresh 是
           同步等待的在线 URL 访问，此前该窗口无任何加载指示且 items 为空，
           空态文案「没有找到匹配的人格」先行渲染，用户以为界面卡死） -->
      <div v-if="loading" style="text-align: center; padding: var(--space-8);">
        <div class="spinner spinner-lg" style="margin: 0 auto;"></div>
        <p style="margin-top: var(--space-3); color: var(--text-secondary); font-size: var(--text-sm);">正在加载人格超市...</p>
      </div>

      <!-- ===== 推荐橱窗（服务端策展顺序；行 = 方向/职能，卡 = 同权共享卡片） ===== -->
      <template v-else-if="activeTab === 'recommend'">
        <div v-if="recommendRows.length === 0" style="text-align: center; padding: var(--space-8); color: var(--text-secondary);">
          暂无推荐内容，请切换到「全部人格」浏览
        </div>
        <div v-for="row in recommendRows" :key="row.key" class="rec-section">
          <div class="rec-section-header">
            <h3 class="rec-section-title">{{ row.title }}</h3>
            <span v-if="row.tier" class="badge badge-neutral" title="该职能建议的模型档位（professions 目录权威数据）">建议模型 {{ tierLabel(row.tier) }}</span>
            <button
              v-if="row.division"
              class="btn btn-sm rec-view-all"
              @click="viewAllDivision(row.division)"
            >查看该类全部 →</button>
          </div>
          <div class="rec-row">
            <PersonaShopCard
              v-for="p in row.cards"
              :key="p.id"
              :p="p"
              class="rec-card"
              :busy-download="downloading"
              :busy-hire="hiring"
              :busy-hire-expert="hiringExpert"
              @preview="showPreview"
              @download="downloadPersona"
              @hire="hirePersona"
              @hire-expert="hireExpert"
            />
          </div>
        </div>
      </template>

      <!-- ===== 全部人格（agency 目录源；builtin 条目由内置页专列，网格过滤） ===== -->
      <template v-else-if="activeTab === 'all'">
        <!-- Search bar -->
        <div class="shop-search-bar">
          <input
            class="form-input"
            v-model="searchQuery"
            placeholder="搜索人格名称或关键词..."
            @keyup.enter="search"
          >
          <button class="btn btn-primary" @click="search">搜索</button>
        </div>

        <!-- Division filter pills -->
        <div class="filter-pills">
          <span
            class="filter-pill"
            :class="{ active: !selectedDivision }"
            @click="setDivision('')"
          >全部</span>
          <span
            v-for="d in divisions"
            :key="d"
            class="filter-pill"
            :class="{ active: selectedDivision === d }"
            @click="setDivision(d)"
          >{{ d }}</span>
        </div>

        <!-- Results count -->
        <div v-if="allPageItems.length > 0" class="shop-count">
          共 {{ allPageItems.length }} 个人格
        </div>

        <!-- Card grid（共享卡片 + 推荐来源轻量角标；builtin 条目过滤——独立页拍板） -->
        <div v-if="allPageItems.length > 0" class="shop-grid">
          <PersonaShopCard
            v-for="p in allPageItems"
            :key="p.id"
            :p="p"
            show-recommend-chips
            :busy-download="downloading"
            :busy-hire="hiring"
            :busy-hire-expert="hiringExpert"
            @preview="showPreview"
            @download="downloadPersona"
            @hire="hirePersona"
            @hire-expert="hireExpert"
          />
        </div>

        <!-- Empty -->
        <div v-if="allPageItems.length === 0" style="text-align: center; padding: var(--space-8); color: var(--text-secondary);">
          没有找到匹配的人格
        </div>
      </template>

      <!-- ===== 内置人格专列（自建人格独立页；首次进入懒加载，客户端搜索；
           源徽标标注在线 / 编译期内嵌兜底） ===== -->
      <template v-else>
        <div class="shop-search-bar">
          <input
            class="form-input"
            v-model="builtinSearch"
            placeholder="筛选内置人格（名称 / 描述关键词）..."
          >
          <button
            class="btn"
            title="重新拉取仓库内置人格清单"
            :disabled="loading"
            @click="loadBuiltin(true)"
          >刷新</button>
        </div>
        <!-- 计数/空态挂 builtinLoaded（2026-11-15 逐行复查修）：挂载刷新
             窗口内切进来时 loadBuiltin 被共享 loading 守卫吞掉，从未加载态
             不得显「共 0 个人格」+「没有匹配」假空态（3df3f89e 同类教训） -->
        <div v-if="builtinLoaded" class="shop-count">
          <span
            v-if="builtinSource === 'embedded'"
            class="builtin-offline-note"
          >⚠ 上游不可达，当前为编译期内嵌兜底（{{ builtinSnapshotLabel }}） · </span>共 {{ builtinFiltered.length }} 个人格
        </div>
        <div v-if="builtinFiltered.length > 0" class="shop-grid">
          <PersonaShopCard
            v-for="p in builtinFiltered"
            :key="p.id"
            :p="p"
            show-recommend-chips
            :busy-download="downloading"
            :busy-hire="hiring"
            :busy-hire-expert="hiringExpert"
            @preview="showPreview"
            @download="downloadPersona"
            @hire="hirePersona"
            @hire-expert="hireExpert"
          />
        </div>
        <div v-else-if="builtinLoaded" style="text-align: center; padding: var(--space-8); color: var(--text-secondary);">
          没有匹配的内置人格
        </div>
        <div v-else style="text-align: center; padding: var(--space-8); color: var(--text-secondary);">
          内置人格清单尚未加载，点击上方「刷新」拉取
        </div>
      </template>

      <!-- Preview modal -->
      <div v-if="previewPersona" class="modal-backdrop" @click.self="closePreview">
        <div class="modal preview-modal">
          <div class="modal-header">
            <h3>{{ previewPersona.emoji }} {{ previewPersona.name }}</h3>
            <button class="modal-close" @click="closePreview">&times;</button>
          </div>

          <!-- Preview tabs -->
          <div class="preview-tabs">
            <button
              class="preview-tab"
              :class="{ active: previewTab === 'original' }"
              @click="previewTab = 'original'"
            >原始内容</button>
            <button
              class="preview-tab"
              :class="{ active: previewTab === 'converted' }"
              @click="previewTab = 'converted'"
            >转换预览</button>
          </div>

          <!-- Fixed-height body -->
          <div class="preview-body">
            <!-- Loading -->
            <div v-if="previewLoading" style="text-align: center; padding: var(--space-8);">
              <div class="spinner" style="margin: 0 auto;"></div>
            </div>

            <!-- Tab 1: Original — full markdown render -->
            <template v-else-if="previewTab === 'original'">
              <div v-if="previewPersona.raw" class="markdown-body" v-html="renderMd(previewPersona.raw)"></div>
              <div v-else style="text-align: center; padding: var(--space-4); color: var(--text-secondary);">
                暂无内容
              </div>
            </template>

            <!-- Tab 2: Converted files -->
            <template v-else-if="previewTab === 'converted'">
              <div v-if="convertedFileList.length > 0" class="converted-layout">
                <div class="converted-sidebar">
                  <div
                    v-for="(f, idx) in convertedFileList"
                    :key="idx"
                    class="converted-file-item"
                    :class="{ active: selectedConverted === idx }"
                    @click="selectedConverted = idx"
                  >{{ f.name }}</div>
                </div>
                <div class="converted-content">
                  <div v-if="convertedFileList[selectedConverted]?.name === 'PERSONA.json'" class="json-block" v-html="escapeHtml(convertedFileList[selectedConverted].content)"></div>
                  <div v-else class="markdown-body" v-html="renderMd(convertedFileList[selectedConverted].content)"></div>
                </div>
              </div>
              <div v-else style="text-align: center; padding: var(--space-4); color: var(--text-secondary);">
                暂无转换数据
              </div>
            </template>
          </div>

          <div class="modal-footer">
            <button class="btn" @click="closePreview">关闭</button>
            <button
              v-if="!previewPersona.installed"
              class="btn btn-primary"
              @click="downloadPersona(previewPersona)"
              :disabled="downloading === previewPersona.id"
            >
              {{ downloading === previewPersona.id ? '下载中...' : '下载' }}
            </button>
            <button
              v-if="clusterEnabled"
              class="btn"
              :class="{ 'btn-primary': isHireEnabled(previewPersona) }"
              :title="hireTitleOf(previewPersona)"
              :disabled="!isHireEnabled(previewPersona) || hiring === previewPersona.id"
              @click="hirePersona(previewPersona)"
            >
              {{ hiring === previewPersona.id ? '聘用中...' : '集群聘用' }}
            </button>
            <button
              class="btn"
              title="将此人格雇佣为本机专家子代理（在【专家Agent】页管理）；原文本地缺席时自动在线拉取保存"
              :disabled="hiringExpert === previewPersona.id"
              @click="hireExpert(previewPersona)"
            >
              {{ hiringExpert === previewPersona.id ? '雇佣中...' : '雇佣专家' }}
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { escapeHtml, renderMarkdownHtml } from '../utils/markdown'
import { useWSAPI } from '../composables/useWSAPI'
import { useToast } from '../composables/useToast'
import PersonaShopCard, { type ShopPersona, isHireEnabled, hireTitleOf } from '../components/PersonaShopCard.vue'

const { request } = useWSAPI()
const toast = useToast()

const activeTab = ref<'recommend' | 'all' | 'builtin'>('recommend')
const searchQuery = ref('')
const selectedDivision = ref('')
// 全部页 = agency 目录源；内置人格独立专列（2026-11-15 拍板不掺合），
// 「内置」pill 相应退场。
const divisions = ref(['开发', '营销', '安全', '创意', '数据', '产品', '通用'])
// shopItems = browse 全量（推荐橱窗 + 全部页共用数据源）；items = 全部页
// 当前渲染列表（browse 时同 shopItems，search 时为过滤结果——搜索不掏空
// 推荐橱窗的卡片）。
const shopItems = ref<ShopPersona[]>([])
const items = ref<ShopPersona[]>([])
const loading = ref(false)
const downloading = ref('')
const hiring = ref('')

// --- 内置人格专列（2026-11-15 独立页拍板）：首次进入懒加载 browse{source:
// "builtin"}；N 小客户端过滤不做服务端 search；源徽标标注在线 / 内嵌兜底 ---
const builtinItems = ref<ShopPersona[]>([])
const builtinLoaded = ref(false)
const builtinSearch = ref('')
const builtinSource = ref<'live' | 'embedded'>('live')
const builtinSnapshotLabel = ref('')

const builtinFiltered = computed(() => {
  const q = builtinSearch.value.trim().toLowerCase()
  if (!q) return builtinItems.value
  return builtinItems.value.filter(p =>
    `${p.name} ${p.id} ${p.description}`.toLowerCase().includes(q)
  )
})

/// 全部页网格 = browse 全量滤掉 builtin（builtin 由独立页专列；推荐行
/// 跨源引用照常消费 shopItems 合并集）。
const allPageItems = computed(() => items.value.filter(p => p.source !== 'builtin'))

async function loadBuiltin(force = false) {
  if (loading.value) return
  if (builtinLoaded.value && !force) return
  loading.value = true
  try {
    const data = await request('persona', 'shop.browse', { source: 'builtin' })
    builtinItems.value = data.items || []
    builtinSource.value = data?.source === 'embedded' ? 'embedded' : 'live'
    builtinSnapshotLabel.value = data?.snapshot_info?.label || ''
    builtinLoaded.value = true
  } catch (e: any) {
    toast.error('加载内置人格失败: ' + (e.message || e))
  } finally {
    loading.value = false
  }
}

// --- 数据源降级态（后端 shop.* 响应的 source / snapshot_info 字段） ---
// 设计拍板（2026-11-12 附加资源 C-ABI 插件）：GitHub 上游不可达时后端自动
// 降级内置快照，前端只负责把来源状态标出来——live = 在线；snapshot =
// 内置快照（commit 前 7 位 @ 日期）。重新拉数据 = 刷新失败后切换标签页
// 再切回来（remount 重新触发 refresh + browse）。
interface SnapshotInfo {
  commit: string
  fetched_at: string
  file_count: number
  label: string
}
const dataSource = ref<'live' | 'snapshot'>('live')
const snapshotInfo = ref<SnapshotInfo | null>(null)

const sourceLabel = computed(() => {
  if (dataSource.value === 'snapshot' && snapshotInfo.value) {
    return `Agency Agents · 内置快照（${snapshotInfo.value.label}）`
  }
  return 'Agency Agents · 在线'
})

/// 从 shop.* 响应捕获数据源状态（browse/search 共用）。
function applySource(data: any) {
  dataSource.value = data?.source === 'snapshot' ? 'snapshot' : 'live'
  snapshotInfo.value = data?.snapshot_info || null
}

// 集群聘用依赖 cluster WSAPI（裁剪构建可关；shop.browse/download 不依赖恒在）
// ——与 router 的 VITE_FEATURE_CLUSTER 视图门控同款（web/.env 由 customize 生成）。
// 雇佣专家（shop.hire_expert）不依赖 cluster——本机专家子代理，恒可用。
const clusterEnabled = import.meta.env.VITE_FEATURE_CLUSTER !== 'false'
const hiringExpert = ref('')
const previewPersona = ref<ShopPersona | null>(null)
const previewLoading = ref(false)
const selectedConverted = ref(0)
const previewTab = ref('original')

// --- 推荐橱窗（服务端策展单一真相源：顺序/内容/tier 注记全由 browse 响应
// 的 recommend_sections 承载，前端零策展逻辑；ref 解析到 shopItems，缺席
// 条目（防死链丢弃）由服务端保证不出现） ---
interface RecommendSection {
  key: string
  title: string
  tier: string | null
  division: string | null
  refs: string[]
}
const recommendSections = ref<RecommendSection[]>([])

/// ref（`source/id`）→ 卡片数据；找不到对应条目的 ref 跳过（双保险）。
const recommendRows = computed(() => {
  const byRef = new Map(shopItems.value.map(p => [`${p.source}/${p.id}`, p]))
  return recommendSections.value
    .map(s => ({
      key: s.key,
      title: s.title,
      tier: s.tier,
      division: s.division,
      cards: s.refs.map(r => byRef.get(r)).filter((p): p is ShopPersona => !!p),
    }))
    .filter(row => row.cards.length > 0)
})

/// professions CATALOG min_tier 中文注记（仅六职能行有值）。
function tierLabel(tier: string): string {
  return tier === 'big' ? '旗舰档' : tier === 'mini' ? '轻量档' : '标准档'
}

/// 「查看该类全部 →」：跳全部页并预置 division 过滤（跨域方向行无 division
/// 不渲染跳转——细分入口统一由推荐页承担）。
function viewAllDivision(division: string) {
  switchTab('all')
  setDivision(division)
}

function switchTab(tab: 'recommend' | 'all' | 'builtin') {
  activeTab.value = tab
  // 内置页懒加载（首次进入拉一次；刷新按钮 force 重拉）。
  if (tab === 'builtin') {
    loadBuiltin()
  }
}

const convertedFileList = computed(() => {
  if (!previewPersona.value?.converted) return []
  return Object.entries(previewPersona.value.converted).map(([name, content]) => ({ name, content }))
})

function renderMd(text: string): string {
  if (!text) return ''
  return renderMarkdownHtml(text)
}

// items 请求代际计数（BUG-20261120-032）：search（Enter/按钮，均无
// disabled 闸）与 browse（division pill/空关键词回车）可在对方在飞期间
// 触发——shop.* 走远程 GitHub 上游（秒级窗口），慢的旧回包后到会把已
// 渲染的新列表覆盖成旧请求的结果（不自愈）。browse/search 各取自增代际，
// 回包只在自己仍是最新代际时落盘与清除 loading；慢回包整体丢弃。
let itemsSeq = 0

async function browse() {
  const seq = ++itemsSeq
  loading.value = true
  try {
    const data = await request('persona', 'shop.browse', {
      division: selectedDivision.value || undefined,
    })
    if (seq !== itemsSeq) return
    applySource(data)
    items.value = data.items || []
    recommendSections.value = data.recommend_sections || []
    // shopItems 只承载未过滤全量（2026-11-15 七轮复查修）：它是推荐橱窗
    // ref 解析的目录集（recommendRows byRef），division 过滤态 items 是
    // 子集——若覆写，切回推荐 TAB 时指向其他 division 的推荐卡集体失联
    // （橱窗被静默裁剪：pill 点一下再切回来，多数行直接消失）。搜索本就
    // 不动 shopItems（下方 search 只写 items）——两处口径对齐。过滤态
    // 下载/雇佣的安装态翻转由调用点就地补丁（不等下次全量 browse）。
    if (!selectedDivision.value) shopItems.value = items.value
  } catch (e: any) {
    if (seq !== itemsSeq) return
    toast.error('加载失败: ' + (e.message || e))
  } finally {
    if (seq === itemsSeq) loading.value = false
  }
}

async function search() {
  if (!searchQuery.value.trim()) {
    return browse()
  }
  const seq = ++itemsSeq
  loading.value = true
  try {
    const data = await request('persona', 'shop.search', {
      query: searchQuery.value,
    })
    if (seq !== itemsSeq) return
    applySource(data)
    items.value = data.items || []
  } catch (e: any) {
    if (seq !== itemsSeq) return
    toast.error('搜索失败: ' + (e.message || e))
  } finally {
    if (seq === itemsSeq) loading.value = false
  }
}

async function downloadPersona(p: ShopPersona) {
  downloading.value = p.id
  try {
    await request('persona', 'shop.download', { id: p.id, source: p.source })
    toast.success('下载成功！若要使用新人格，需要去人格标签页里面启用')
    if (previewPersona.value && previewPersona.value.id === p.id) {
      previewPersona.value.installed = true
    }
    await browse()
    // 内置页已加载过则同步刷新安装态（独立页数据源独立于 browse）。
    if (builtinLoaded.value) await loadBuiltin(true)
    // division 过滤态 browse 不覆写 shopItems（全量集）——就地补安装态，
    // 防推荐卡 stale（非过滤态 browse 已带最新全量，此处幂等）。
    for (const it of shopItems.value) {
      if (it.id === p.id && it.source === p.source) it.installed = true
    }
  } catch (e: any) {
    toast.error('下载失败: ' + (e.message || e))
  } finally {
    downloading.value = ''
  }
}

async function hirePersona(p: ShopPersona) {
  hiring.value = p.id
  try {
    const data = await request('persona', 'shop.hire', { id: p.id, source: p.source })
    toast.success(`「${p.name}」已聘入集群` + (data?.note || ''))
    // builtin 免下载直聘用物化 personas/<id>/ → 安装态翻转为已安装，就地
    // 同步两套陈列集（agency 聘用前置下载已 installed=true，此处幂等）。
    if (p.source === 'builtin') {
      for (const it of [...shopItems.value, ...builtinItems.value]) {
        if (it.id === p.id) it.installed = true
      }
    }
    if (previewPersona.value && previewPersona.value.id === p.id) {
      closePreview()
    }
  } catch (e: any) {
    toast.error('集群聘用失败: ' + (e.message || e))
  } finally {
    hiring.value = ''
  }
}

async function hireExpert(p: ShopPersona) {
  hiringExpert.value = p.id
  try {
    // 取材链本地 raw.md → builtin 内嵌 → 在线，对前端透明（无需带 source）。
    await request('persona', 'shop.hire_expert', { id: p.id })
    toast.success(`「${p.name}」已雇佣为专家，可在【专家Agent】页管理`)
    if (previewPersona.value && previewPersona.value.id === p.id) {
      closePreview()
    }
  } catch (e: any) {
    toast.error('雇佣专家失败: ' + (e.message || e))
  } finally {
    hiringExpert.value = ''
  }
}

async function showPreview(p: ShopPersona) {
  previewPersona.value = p
  previewLoading.value = true
  previewTab.value = 'original'
  selectedConverted.value = 0
  try {
    const data = await request('persona', 'shop.preview', { id: p.id, source: p.source })
    // 迟到响应丢弃（2026-11-15 八轮复查修）：加载中关窗（× 与遮罩点击均
    // 未禁用，shop.preview 在线拉取可达 30s 超时窗）或换卡后 previewPersona
    // 引用已变——回填会让已关闭的弹窗无端重开。引用同一性是本轮仍有效的
    // 唯一判据（同一张卡再点是新一轮，引用相同照常生效）。
    if (previewPersona.value !== p) return
    previewLoading.value = false
    previewPersona.value = {
      ...p,
      name: data.name || p.name,
      emoji: data.emoji || p.emoji,
      installed: data.installed ?? p.installed,
      raw: data.raw || undefined,
      converted: data.converted || undefined,
    }
  } catch (e: any) {
    // 已关窗/换卡：失败随过期请求一并丢弃（用户已取消关心）。
    if (previewPersona.value !== p) return
    previewLoading.value = false
    toast.error('加载预览失败: ' + (e.message || e))
  }
}

function closePreview() {
  previewPersona.value = null
}

function setDivision(d: string) {
  selectedDivision.value = d
  browse()
}

onMounted(async () => {
  // 挂载即刷新上游（拉最新）。失败 = 诚实报错（后端 shop.refresh 语义：
  // 上游不可达不再静默吞掉），badge 随后由 browse 的降级响应呈现快照态。
  // 刷新 + 浏览整个窗口都挂 loading 转圈——shop.refresh 在线拉 GitHub
  // （30s 客户端超时窗），期间 items 为空，不挂转圈就是一片「假空态」。
  loading.value = true
  try {
    await request('persona', 'shop.refresh', {})
  } catch (e: any) {
    toast.error('刷新人格超市失败: ' + (e.message || e))
  }
  await browse()
})
</script>

<style scoped>
/* 快照降级态徽标：琥珀色调提醒「数据来自内置快照」 */
.shop-source-badge {
  margin-left: var(--space-3);
}

.shop-source-snapshot {
  color: var(--warning, #d97706);
}

/* 内置页离线兜底注记：琥珀色调与快照徽标同语义（上游不可达） */
.builtin-offline-note {
  color: var(--warning, #d97706);
}

.shop-search-bar {
  display: flex;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
}

.shop-search-bar .form-input {
  max-width: 400px;
}

.filter-pills {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  margin-bottom: var(--space-3);
}

.filter-pill {
  padding: 4px 12px;
  border-radius: 9999px;
  font-size: var(--text-sm);
  cursor: pointer;
  border: 1px solid var(--border);
  background: var(--surface);
  color: var(--text-secondary);
  transition: all 0.15s;
}

.filter-pill:hover {
  border-color: var(--primary);
  color: var(--primary);
}

.filter-pill.active {
  background: var(--primary);
  border-color: var(--primary);
  color: white;
}

.shop-count {
  font-size: var(--text-sm);
  color: var(--text-secondary);
  margin-bottom: var(--space-3);
}

.shop-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: var(--space-4);
}

/* --- 推荐橱窗（一行一个方向，行内横排卡片；横滚不折叠） --- */
.rec-section {
  margin-bottom: var(--space-5);
}

.rec-section-header {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-2);
}

.rec-section-title {
  margin: 0;
  font-size: var(--text-base);
}

.rec-view-all {
  margin-left: auto;
}

.rec-row {
  display: flex;
  gap: var(--space-4);
  overflow-x: auto;
  padding-bottom: var(--space-2);
}

.rec-card {
  flex: 0 0 280px;
  min-width: 280px;
}

/* --- Preview modal (fixed size) --- */
.preview-modal {
  max-width: 960px;
  width: 90vw;
  display: flex;
  flex-direction: column;
}

.preview-tabs {
  display: flex;
  border-bottom: 1px solid var(--border);
  padding: 0 var(--space-3);
  flex-shrink: 0;
}

.preview-tab {
  padding: var(--space-2) var(--space-4);
  font-size: var(--text-sm);
  color: var(--text-secondary);
  border: none;
  background: none;
  cursor: pointer;
  border-bottom: 2px solid transparent;
  transition: all 0.15s;
}

.preview-tab:hover {
  color: var(--text-primary);
}

.preview-tab.active {
  color: var(--primary);
  border-bottom-color: var(--primary);
  font-weight: 500;
}

/* Fixed-height scrollable body */
.preview-body {
  height: 65vh;
  overflow-y: auto;
  padding: var(--space-4);
}

/* --- Converted layout (sidebar + content) --- */
.converted-layout {
  display: flex;
  gap: var(--space-3);
  height: 100%;
}

.converted-sidebar {
  flex-shrink: 0;
  width: 140px;
  border-right: 1px solid var(--border);
  overflow-y: auto;
}

.converted-file-item {
  padding: var(--space-2) var(--space-3);
  cursor: pointer;
  font-size: var(--text-sm);
  color: var(--text-secondary);
  border-left: 2px solid transparent;
  transition: all 0.15s;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  font-family: var(--font-mono);
}

.converted-file-item:hover {
  color: var(--text-primary);
  background: var(--surface);
}

.converted-file-item.active {
  color: var(--primary);
  border-left-color: var(--primary);
  background: var(--surface);
  font-weight: 500;
}

.converted-content {
  flex: 1;
  overflow-y: auto;
  min-width: 0;
}

/* --- Markdown rendering --- */
.preview-body :deep(.markdown-body) {
  font-size: var(--text-sm);
  line-height: 1.6;
}

.preview-body :deep(.markdown-body h1),
.preview-body :deep(.markdown-body h2),
.preview-body :deep(.markdown-body h3) {
  margin-top: var(--space-3);
  margin-bottom: var(--space-2);
}

.preview-body :deep(.markdown-body h1) { font-size: 1.25rem; }
.preview-body :deep(.markdown-body h2) { font-size: 1.1rem; }
.preview-body :deep(.markdown-body h3) { font-size: 1rem; }

.preview-body :deep(.markdown-body code) {
  background: var(--surface);
  padding: 1px 4px;
  border-radius: 3px;
  font-size: 0.9em;
}

.preview-body :deep(.markdown-body pre) {
  background: var(--surface);
  padding: var(--space-3);
  border-radius: var(--radius-md);
  overflow-x: auto;
}

.preview-body :deep(.markdown-body pre code) {
  background: none;
  padding: 0;
}

.preview-body :deep(.markdown-body ul),
.preview-body :deep(.markdown-body ol) {
  padding-left: var(--space-4);
}

.preview-body :deep(.markdown-body blockquote) {
  border-left: 3px solid var(--primary);
  margin: var(--space-2) 0;
  padding: var(--space-2) var(--space-3);
  color: var(--text-secondary);
}

.preview-body :deep(.markdown-body hr) {
  border: none;
  border-top: 1px solid var(--border);
  margin: var(--space-3) 0;
}

.preview-body :deep(.markdown-body table) {
  border-collapse: collapse;
  width: 100%;
  margin: var(--space-2) 0;
}

.preview-body :deep(.markdown-body th),
.preview-body :deep(.markdown-body td) {
  border: 1px solid var(--border);
  padding: var(--space-1) var(--space-2);
  font-size: 0.9em;
}

.preview-body :deep(.markdown-body th) {
  background: var(--surface);
  font-weight: 500;
}

/* JSON code block */
.preview-body :deep(.json-block) {
  white-space: pre-wrap;
  word-break: break-word;
  font-size: var(--text-sm);
  line-height: 1.6;
  font-family: var(--font-mono);
  background: var(--surface);
  padding: var(--space-4);
  border-radius: var(--radius-md);
  margin: 0;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  padding: var(--space-3);
  border-top: 1px solid var(--border);
  flex-shrink: 0;
}
</style>
