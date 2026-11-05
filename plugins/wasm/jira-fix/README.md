# jira-fix 域插件

WASM 插件**自持形态**（两件套：plugin.toml + guest；照 bugfix-flow 样板工程；来源 = Jira Server/Data Center REST v2）。取 Jira issue 进流水线修复，进展与报告回帖 Jira——发布（PR）不在本插件：Jira 不托管 git，仓库联动交给 gitea 侧插件（见下方「与 gitea 组合」）。

**0.2.0 自持批**（2026-11-03 插件自持写通道）：连接知识进 guest——Jira 直连（http-send + secret-get）不再是宿主 connector 声明；实例配置（base_url / project_key / max_results）与凭据（PAT）由插件**自带的面板「实例设置」表单**写入（config-set ×3 + secret-set）。connector.toml 退役。同一组件双导出（组合 world `plugin-panel-flow-step`）：面板命令通道 + flow 步骤执行体。

## 两件套

| 文件 | 角色 | 说明 |
|---|---|---|
| `plugin.toml` | 声明面 | 页面骨架（五词汇视图 + 实例设置表单）+ flow 剧本（五步，fetch/report 走 `plugin:jira-fix:*` 自报步骤）+ origin/回写声明 + egress/x-secret 声明 + guest 载荷哈希 |
| `guest/` | 血肉面 | 组合 world guest（wasm32-wasip2）：面板命令 `sync` / `jira_get` / `save_settings`；flow 步骤 `fetch`（JQL 检索）/ `comment`（issue 回帖） |

## 剧本（jira-pipeline，五步）

```
plugin:jira-fix:fetch → board_land → agent_fixloop(gate=manual) → check_regress → report
        取（guest 直连）      落（看板单）     修复循环（人工闸）        验收            报告+dossier+回帖
```

与 bugfix-flow 六步全链的差异：**无 publish 步**（Jira 无 PR 概念；报告直接回帖 Jira issue）。

- **guest 自报步骤**：fetch/comment 由 guest `get_step_metadata` 报词表（`fetch` / `comment`），宿主按 `op` 路由（`plugin:jira-fix:<op>`）。取数/回帖 = guest 内 http-send（过帧闸：egress + 预算 + 审计）+ secret-get（PAT 现取现用）。
- **origin 三件套**：`origin-id = "jira/{{config.project_key}}#{{key}}"`（如 `jira/PROJ#PROJ-123`；`{{config.project_key}}` 装载期渲染，缺省 PROJ），board 单按整串精确去重。⚠ plugin fetch 模板禁 `{{connector}}`/`{{arg.*}}`——origin-id 里引用 project_key 必须走 `{{config.*}}`。
- **fetch 取数字段**：Jira search 返回包裹对象（`{issues:[{key, fields:{...}}]}`，非顶层数组）——`key_field`/`title_field`/`body_field` 全走 `$.` JSON path（`$.key` / `$.fields.summary` / `$.fields.description`）。`[CHECK]` 行验收锚约定照旧（写在 Jira description 里）。
- **on-transition**：board 状态转移 → guest `comment` op 回帖 Jira；`origin_meta` 优先，`parse-origin-id`（`after '#'`）兜旧单。
- **issue key 是字符串**（PROJ-123 形态），无整数序号——guest path 段直接字符串替换。

## 自持写通道（实例设置表单）

面板「实例设置」表单（四字段：Jira 基址 / 项目键 / 取数条数上限 / 访问令牌 PAT）→ guest `save_settings` 命令：

- `base_url` / `project_key` / `max_results` → config-set（实例配置；`{{config.*}}` 剧本渲染 + egress `{{config.base_url}}` 热生效——fresh-store-per-call，下帧即新值）。
- `pat` → secret-set（vault `plugin/jira-fix/pat`；只写不读，保存后无法回显）。
- 四字段全可留空 = 保持原值；全空提交 = 页面挂载自动探活，诚实无写入应答。
- `base_url` 无声明缺省（防假缺省 URL）——未设置时各路诚实报错（fetch/sync/jira_get/comment 都会提示先去面板保存）。

单条配置微调也可走插件页「配置」钮（同一实例配置面）；PAT 只能走面板表单。

## 安装（开发机）

```bash
# 1. 构建 guest（wasm32-wasip2 target 需先 rustup target add；
#    ⚠ guest Cargo.toml 的 [profile.release] 必须保持缺省——opt-level="z"/lto
#    会破坏 SDK wire 模块的段裁除，链接期 world 污染，见 Cargo.toml 注释）
cd guest && cargo build --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/jira_fix_plugin.wasm ../plugin.wasm
# 2. 哈希回填 plugin.toml 的 wasm-sha256
sha256sum ../plugin.wasm
# 3. 装配（--allow-unsigned 自担信任；生产先 plugin keygen/sign/trust）
nemesisbot plugin install plugins/wasm/jira-fix --yes --allow-unsigned
nemesisbot plugin enable jira-fix            # 新装默认不加载（安装≠加载）
# 4. 重启 gateway → Dashboard 侧栏【定制】分组出现「Jira 修复流水线」
# 5. 面板「实例设置」表单保存 Jira 基址 + PAT（PAT 落 vault，不再有
#    vault set connector/jira/pat 手工步骤）
```

## 换靶坐标

| 要改什么 | 改哪里 |
|---|---|
| Jira 实例地址 | 面板「实例设置」表单（config `base_url`；egress 随之热生效） |
| 目标项目 | 面板「实例设置」表单（config `project_key`；剧本/origin-id 装载期渲染） |
| 取数条数 | 面板「实例设置」表单（config `max_results`） |
| 导入范围 | `plugin.toml` fetch 步 JQL（当前 `status not in (Done, Closed)`；加组件/标签过滤自行扩） |
| 回写文案 | `on-transition.args.body` / report 步 `args_json` 模板 |

## 与 gitea 组合（发布面）

Jira 管工单、Gitea 管仓库是常见组合：装 bugfix-flow 后，修复循环产出的交付分支/PR 走 gitea 侧流水线；本插件剧本不含 publish——两插件各管一半，看板单是共同执行真相源。

## 诚实边界

- **当前认证 = PAT Bearer**（Jira Server/DC）：guest 全权构造请求后，0.1 的两个 connector 表达力边界（Cloud Basic `base64(email:api_token)`、transitions 嵌套 body）在**能力面解除**——guest 想写就能写，当前实现未带。Cloud / 状态流转要接时改 guest，不改清单。
- **内网靶是产品语义**：manifest `allow-private = true`（自托管 Jira Server/DC 本就常驻企业内网）；egress allowlist 仍钉 `base_url` 精确值，DNS 钉地址（rebinding 防御）照常——放行的是「目标可在私网」，不是「任意私网」。
- **base_url 未配置 = 诚实报错**：不猜默认 URL；面板保存后热生效。
- **PAT 只写不读**：secret-get 只在发请求时现取，面板不回显。
- **confirm/resume 按钮无参**：作用于最近更新的实例；per-instance 走页面上方的确认/恢复表单。
- 面板信任级 = dashboard 登录用户；guest 命令经 `plugins.ui.invoke` 过审计链，guest 内 http-send 过帧闸（egress + 预算 + 审计）。
