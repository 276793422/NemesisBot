# issue-import 轻量工单导入模板（自持缩版样板）

WASM 插件「面板 + 流程剧本 + guest 直连」**两件套**（0.2.0 自持批起；0.1.x 纯声明式形态已退役）：外部工单轮询 → 看板单（origin 去重）+ 状态回帖，guest 缩版两 op 直连 Gitea REST。与 `bugfix-flow` 全管线解耦——那边是「导入→修复→验收→发布→报告」六步全家桶，这边只有「取→落」两步 + 回写声明，作为**换靶起点**（照抄改坐标就是你的工单系统接入，宿主零改动）。

## 两件套

| 文件 | 角色 | 说明 |
|---|---|---|
| `plugin.toml` | 声明面 | 页面骨架（table/timeline/form/actions）+ flow 剧本（fetch→land 两步，双路 plugin 引用）+ origin/回写协议声明 + permissions + config-schema + guest 载荷哈希 |
| `guest/` | 血肉面 | 缩版 guest（wasm32-wasip2，`nemesis-plugin-sdk::export_panel_step!`）：面板命令 `save_settings`（config-set ×4 + secret-set pat）+ 步骤 op `fetch` / `comment`，直连 Gitea REST |

流程编排全在宿主 flow 引擎（`nemesisbot/src/flow_engine.rs`）——guest 无驻留（fresh-store-per-call），只做请求-响应。**领域知识（REST 形态、凭据头、origin_id 格式）只在插件声明面与 guest——宿主零领域代码**。

## 凭据与配置（自持写通道）

- **PAT**：面板「实例设置」表单写入 → vault 别名 `plugin/issue-import/pat`（guest secret-get 现取现用；Gitea 认证头 `Authorization: token <PAT>`——非 Bearer）。0.1.x 的 `connector/gitea/pat` 别名已废弃。
- **实例配置**：base_url（**无缺省**，须含 `/api/v1` 前缀）/ owner / repo / state——面板表单或插件页「配置」钮均可改，剧本 `{{config.*}}` 装载期渲染、egress 热生效。
- 未配置 base_url/PAT 时各路诚实报错——模板即装即载，不猜 URL 不带假缺省。

## 剧本（issue-import，两步 = 轻量形态全集）

```
plugin:issue-import:fetch → board_land
     取（guest 直连）        落（看板单，缺省 auto 无人工闸）
```

- `interval-secs = 600` 轮询 + 面板「同步外部 issue」按钮手动触发。
- **origin 三件套**：`origin-id = "gitea/{{config.owner}}/{{config.repo}}#{{key}}"`（字面 gitea 前缀，渲染产物 `gitea/<owner>/<repo>#<n>` 与 0.1.x 同形），board 按整串精确去重。
- **on-transition**：board 状态转移 → guest `comment` op 回帖；`origin_meta` 优先，`parse-origin-id` 兜旧单（`segment '/' 1 | before '#' | segment '/' 2 | after '#'` 切分原语）。

## 构建 + 安装（开发机）

```bash
# 1. 构建 guest（wasm32-wasip2 target 需先 rustup target add）
cd guest && cargo build --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/issue_import_plugin.wasm ../plugin.wasm
# 2. 哈希回填 plugin.toml 根键 wasm-sha256
sha256sum ../plugin.wasm
# 3. 装配（--allow-unsigned 自担信任；生产先 plugin keygen/sign/trust）
nemesisbot plugin install plugins/wasm/issue-import --yes --allow-unsigned
nemesisbot plugin enable issue-import          # 新装默认不加载（安装≠加载）
# 4. 首次配置：Dashboard 插件页「配置」钮或面板「实例设置」表单——
#    base_url（含 /api/v1）+ PAT 必填；owner/repo/state 有缺省
# 5. 重启 gateway → Dashboard 侧栏【定制】分组出现「Issue 导入流水线」
```

⚠ guest 构建铁律：`guest/Cargo.toml` 的 `[profile.release]` 必须完全缺省（详见该文件注释，与 bugfix-flow/jira-fix 同款）。

## 换靶（宿主零改动）

换成任意工单系统 = 改 guest 的 REST 包装函数（gitea_request/gitea_issues/gitea_comment 三处）+ plugin.toml 的 origin-id 格式与 parse-origin-id 切分表达式 + 回帖 args 形状。要工单落指定 board 项目（任务副本/发布链配套）时给 `land` 步加 `args = { project = "<board 项目名>" }`——缺省落主工作区。

## 诚实边界

- **只导入 + 回帖**：不修不发布（那是 bugfix-flow 的事）；回写面只做 issue_comment（issue 状态流转用平台界面）。
- 去重键 = origin_id 整串精确匹配；外部侧改 issue 标题/正文不重复建单，改编号（key）才会视为新工单。
- 面板信任级 = dashboard 登录用户；guest 内 http-send 走宿主出网闸（egress allowlist 钉 base_url 精确值 + DNS 钉地址）过完整 dispatch 瀑布（安全 8 层同源）。
