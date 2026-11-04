# bugfix-flow 域插件（自持形态样板）

WASM 插件「面板 + 流程剧本 + guest 直连」**两件套**样板。0.2.x 时代的三件套（connector.toml 领域接口）已退役：0.3.0 起插件**自带 Gitea 直连**——guest 以组合 world `plugin-panel-flow-step` 自报步骤（fetch/comment/create_pull/pr_list 四 op，http-send + secret-get 直连 Gitea REST），实例配置与凭据经面板「实例设置」表单自持写入（config-set + secret-set）。宿主不再为本插件提供任何领域代码——这是「插件自持」架构的权威样板（Gitea 换成任意工单/业务系统，剧本换成任意编排，宿主零改动）。

## 两件套

| 文件 | 角色 | 说明 |
|---|---|---|
| `plugin.toml` | 声明面 | 页面骨架（五词汇视图）+ flow 剧本（六原语步骤，三路外部调用全 plugin 引用）+ origin/回写/发布协议声明 + permissions（egress/x-secret）+ config-schema + guest 载荷哈希 |
| `guest/` | 血肉面 | 薄 guest（wasm32-wasip2，`nemesis-plugin-sdk::export_panel_step!` 一键导出）：面板命令 `sync` / `gitea_get` / `save_settings` + 步骤 op `fetch` / `comment` / `create_pull` / `pr_list`，全部直连 Gitea REST |

流程编排全在宿主 flow 引擎（`nemesisbot/src/flow_engine.rs`）——guest 无驻留（fresh-store-per-call），只做请求-响应；这是平台契约不是实现取舍。**领域知识（REST 形态、凭据头、origin_id 格式、PR 协议）只在插件声明面与 guest——宿主只提供通用原语与 token 插值**。

## 凭据与配置（自持写通道）

- **PAT**：面板「实例设置」表单写入 → vault 别名 `plugin/bugfix-flow/pat`（guest secret-get 现取现用；Gitea 认证头是 `Authorization: token <PAT>`——**非 Bearer**）。0.2.x 的 `connector/gitea/pat` 别名已废弃。
- **实例配置**：base_url（**无缺省**，须含 `/api/v1` 前缀）/ owner / repo / state / base_branch——面板表单或插件页「配置」钮均可改（同一实例配置面），剧本 `{{config.*}}` 装载期渲染、egress `{{config.base_url}}` 热生效。
- 未配置 base_url/PAT 时各路诚实报错——模板即装即载，不猜 URL 不带假缺省。

## 剧本（bugfix-pipeline，六步 = 六原语全集）

```
plugin:bugfix-flow:fetch → board_land → agent_fixloop(gate=manual) → check_regress → publish(gate=manual) → report
          取（guest 直连）      落（看板单）     修复循环（人工闸）          验收            发布 PR（人工闸，缺口续行）        报告+dossier+回帖
```

- `gate=manual`：实例停 `awaiting_gate`，面板确认按钮放行（引擎下个 tick 推进）；fixloop 与 publish 共用同一按钮。
- **origin 三件套（C-1 导入循环）**：`origin-type`/`origin-id`/`origin-meta` 声明建单坐标（`gitea/<owner>/<repo>#<n>` 形态），board 单按整串精确去重。plugin fetch 模板的身份 token 表 = `{{key}}`（条目键 = key_field 字段值）+ `{{config.*}}`（实例配置）；`{{connector}}`/`{{arg.*}}` 已禁（connector 退役）。
- **on-transition（C-2 状态回写）**：board 状态转移 → guest `comment` op 回帖 Gitea；`origin_meta` 优先，`parse-origin-id` 兜 v1 旧单（`segment '/' 1 | before '#' | segment '/' 2 | after '#'` 切分原语；origin-id 0.3.0 改字面 `gitea` 前缀但渲染产物与 0.2.x 同形，表达式不变）。
- **publish 协议（C-3）**：`create_op`/`create_args`/`result_ref`/`reconcile_op`/`reconcile_args`/`reconcile_match` 全在模板，双 op 均 plugin 引用；推交付分支 `nemesis/task-<task_id>` + 建 PR + Found/Absent/Inconclusive 三态对账（reconcile 翻页由宿主注入 limit/page 数字基参）。`on-fail=continue`：发布失败记缺口不停车，report 照常出报告。
- `report`：dossier 导出 + `{{report_path}}` token 经 guest `comment` op 回帖 Gitea issue。

## 构建 + 安装（开发机）

```bash
# 1. 构建 guest（wasm32-wasip2 target 需先 rustup target add）
cd guest && cargo build --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/bugfix_flow_plugin.wasm ../plugin.wasm
# 2. 哈希回填 plugin.toml 根键 wasm-sha256
sha256sum ../plugin.wasm
# 3. 装配（--allow-unsigned 自担信任；生产先 plugin keygen/sign/trust）
nemesisbot plugin install plugins/wasm/bugfix-flow --yes --allow-unsigned
nemesisbot plugin enable bugfix-flow          # 新装默认不加载（安装≠加载）
# 4. 首次配置：Dashboard 插件页「配置」钮或面板「实例设置」表单——
#    base_url（含 /api/v1）+ PAT 必填；owner/repo/state/base_branch 有缺省
# 5. 重启 gateway → Dashboard 侧栏【定制】分组出现「BUG 修复流水线」
```

⚠ guest 构建铁律：`guest/Cargo.toml` 的 `[profile.release]` 必须完全缺省——opt-level="z"/lto 等任何覆盖会破坏 SDK rlib wire 模块的 component-type 段裁除（详见该文件注释）。

## 诚实边界

- **issue 状态流转（open↔closed）未接**：回写面只做 issue_comment（进展可见）——状态流转用平台/Gitea 界面，需要时把 close 逻辑加进 guest op + on-transition/on_fail 分支。
- **confirm/resume 按钮无参**（v1 按钮不带 args）：作用于最近更新的实例；per-instance 走页面上方的确认/恢复表单（form 字段 `instance`）。
- **git 推送仍在宿主侧**：publish 步的交付分支推送由宿主 git_repo（git2 + vault 凭据）执行，guest 只负责建 PR/对账——`publish.rs` 的 owner/repo args 仍必填。
- 面板信任级 = dashboard 登录用户；guest 命令经 `plugins.ui.invoke` 过审计链，guest 内 http-send 走宿主出网闸（egress allowlist 钉 base_url 精确值 + DNS 钉地址）过完整 dispatch 瀑布（安全 8 层同源）。
