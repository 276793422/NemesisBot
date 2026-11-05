# sc-bugfix 域插件（SC BUG 修复流水线）

WASM 插件**自持形态**（两件套：plugin.toml + guest；照 jira-gitlab-flow 样板工程，扩展为**双代码托管源**）。全链：**Jira（Server/DC）取 BUG → 仓副本 clone-or-pull（GitLab / Gerrit 按 `code_host` 二选一）→ 看板落项目修复循环 → 发布（GitLab MR / Gerrit change）→ Jira 回帖上报**。域知识全在模板声明 + guest（宿主只提供通用原语：board_land / agent_fixloop / check_regress / repo_sync / publish / report + token 插值）。

同一组件双导出（组合 world `plugin-panel-flow-step`）：面板命令通道（`sync` / `jira_get` / `save_settings`）+ flow 步骤执行体（`fetch` / `comment` / `report` / `create_mr` / `mr_list` / `change_list`）。

## 两件套

| 文件 | 角色 | 说明 |
|---|---|---|
| `plugin.toml` | 声明面 | 页面骨架（表/时间线/详情/三表单/动作钮）+ flow 剧本（双臂九步）+ origin/回写/publish/repo_sync 声明 + egress/x-secret + guest 载荷哈希 |
| `guest/` | 血肉面 | 组合 world guest（wasm32-wasip2）：Jira / GitLab / Gerrit REST 三直连（http-send 过帧闸：egress + 预算 + 审计；secret-get 现取现用） |

## 剧本（sc-bugfix-pipeline，双臂九步）

```
plugin:fetch → repo_sync(gitlab臂) ⟍
                                      → board_land → agent_fixloop(gate=manual) → check_regress → publish(gitlab臂) ⟍
             repo_sync(gerrit臂) ⟋                                                                              publish(gerrit臂) ⟋ → report
  Jira 检索     仓副本同步（按 code_host 裁剪）  落看板项目    修复循环（唯一人工闸）      验收          建MR+缺口对账 / refs/for+change对账    Jira 回帖上报
```

- **双臂裁剪**：`sync_gitlab`/`sync_gerrit` 与 `publish_gitlab`/`publish_gerrit` 各带 `when = "{{config.code_host}} == <源>"`——装载期渲染成字面比较，假臂 = Skipped 非失败（同 tick 推进，`artifacts.skipped_when` 留痕）。一组件一实例一托管源，不混用。
- **fetch（guest 直连）**：Jira search（JQL 内联 `project = {{config.jira_project_key}}`）；`key/title/body` 三字段是宿主物化坐标（`$.` JSON path，Jira search 包裹对象：key 顶层、正文藏 fields 下）。
- **repo_sync（宿主原语）**：项目目录 clone-or-pull（目录 = 看板项目注册表 `directory` 字段唯一真相源，**不自动注册**）。GitLab 凭据 = vault `plugin/sc-bugfix/pat`（cred_user 取 `gitlab_owner`，HTTPS+PAT 任意非空用户名）；Gerrit 凭据 = `plugin/sc-bugfix/gerrit_http_pass`（HTTP Basic，cred_user 取 `gerrit_username`，URL 带 `/a/` 认证前缀）。失败 = 停车（没法修复）。
- **land → fixloop**：落看板项目 + 修复循环（`args = { target = "self" }` 单机自持执行面，一等声明无回退）。**人工闸只有这一道**（gate=manual）。
- **publish_gitlab（自动不停）**：推交付分支 `nemesis/task-<task_id>` + MR 创建（`create_mr`）+ 缺口对账三态（`mr_list` 按 `$.source_branch` 匹配回读）。`on-fail=continue`：发布失败 = 缺口记账不停车，报告照常出。
- **publish_gerrit（自动不停）**：推 `refs/for/<target>`（宿主 Gerrit 臂：单 commit + Change-Id 页脚，Change-Id 由 task_id 确定性派生——重试 = 同 change 新 patchset 不重复建）+ `change:<Change-Id>` query-as-create（create 即检索，经对账保守分支收敛）+ `$.change_id` 对账匹配。`change_list` 归一投影 ChangeInfo（number/web_url 按基址构造）。
- **report（guest report op）**：`jira_transition_name` **留空 = 只回帖不转状态**（诚实跳过）；非空时幂等三步——①已在目标态跳过 ②comments 同文去重回帖 ③transitions 按 name/to.name 不分大小写匹配转状态。转失败 = failed 注明「回帖已发出」+ 可用名单。
- **on-transition**：board 状态转移 → guest `comment` op 回帖 Jira（`origin_meta` 优先，`parse-origin-id` 兜旧单）。

## 自持写通道（实例设置表单）

面板「实例设置」表单（16 config + 4 secret 字段）→ guest `save_settings` 命令：

- **16 config**：`code_host`（gitlab|gerrit，值域校验）/ 三个 base_url（无缺省，去尾斜杠归一）/ `jira_project_key` / `jira_auth_mode`（pat|basic，值域校验）/ `jira_username` / `jira_transition_name` / `gitlab_owner` / `gitlab_repo` / `gitlab_target_branch` / `gerrit_username` / `gerrit_project` / `gerrit_target_branch` / `board_project` / `proxy`。
- **4 secret**：`jira_pat`（Bearer）/ `jira_password`（Basic）/ `pat`（GitLab PRIVATE-TOKEN **兼 git 推送密码**——名字被宿主 publish 推送凭据推导钉死 `plugin/sc-bugfix/pat`）/ `gerrit_http_pass`（Gerrit HTTP Password）。只写不读，保存后无法回显。
- 全可留空 = 保持原值；全空提交 = 页面挂载自动探活，诚实无写入应答。
- 枚举字段（code_host / jira_auth_mode）**先验后写**：值域外 = 整批拒绝不落半截配置。

## Jira 双认证形态

`jira_auth_mode` 配置（缺省 `pat`）：

- `pat`：Bearer 令牌（`jira_pat` secret → `Authorization: Bearer <pat>`）。
- `basic`：账号密码（`jira_username` config + `jira_password` secret → `Authorization: Basic base64(user:pass)`；编码器 std 内联）。
- 未知值 = 诚实报错不猜（凭据面错配宁可响不可静默）。

## Gerrit REST 形态

- HTTP Basic（`gerrit_username` + `gerrit_http_pass`——HTTP Password 在 Gerrit 设置里生成，非登录密码）。
- 响应带 XSSI 魔法前缀 `)]}'`，guest 剥掉再解析。
- change 检索 `GET /a/changes/?q=<query>&n=<limit>&S=<(page-1)*limit>`（宿主 reconcile 翻页直投，limit/page 注入 JSON 数字，arg_any 双形态兼容）。
- 归一投影：`_number` → `number`、web_url 无独立字段按 `{base}/c/{project}/+/{_number}` 构造。

## 安装（开发机）

```bash
# 1. 构建 guest（wasm32-wasip2 target 需先 rustup target add；
#    ⚠ guest Cargo.toml 的 [profile.release] 必须保持缺省——opt-level="z"/lto
#    会破坏 SDK wire 模块的段裁除，链接期 world 污染，见 Cargo.toml 注释）
cd guest && cargo build --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/sc_bugfix_plugin.wasm ../plugin.wasm
# 2. 哈希回填 plugin.toml 的 wasm-sha256
sha256sum ../plugin.wasm
# 3. 装配（--allow-unsigned 自担信任；生产先 plugin keygen/sign/trust）
nemesisbot plugin install plugins/wasm/sc-bugfix --yes --allow-unsigned
nemesisbot plugin enable sc-bugfix    # 新装默认不加载（安装≠加载）
# 4. 重启 gateway → Dashboard 侧栏【定制】分组出现「SC BUG 修复流水线」
# 5. 面板「实例设置」保存基址 + 凭据（按 code_host 二选一，只填选中源的
#    坐标即可）；看板侧注册项目（与 board_project 配置同名、directory 指向
#    仓副本落点）→ 按钮触发首次取数或等 interval
```

## 换靶坐标

| 要改什么 | 改哪里 |
|---|---|
| 托管源切换 | 面板「实例设置」（`code_host` = gitlab 或 gerrit） |
| Jira 地址 / 认证形态 | 面板「实例设置」（`jira_base_url` / `jira_auth_mode` + 对应凭据字段） |
| GitLab/Gerrit 坐标 | 面板「实例设置」（对应 `*_base_url` / `*_owner|username` / `*_repo|project`） |
| MR/change 目标分支 / 完成态 | 面板「实例设置」（`*_target_branch` / `jira_transition_name`） |
| 导入范围 | `plugin.toml` fetch 步 JQL（换靶改 `jira_project_key` 即换 JQL——JQL 不做 config 键，单遍渲染不支持跨键引用） |
| 看板项目绑定 | 面板「实例设置」（`board_project`；须与看板注册项目同名且带 directory） |
| 回写/报告文案 | `on-transition.args.body` / publish `create_args` / report `args_json` 模板 |

## 与 jira-fix / bugfix-flow / jira-gitlab-flow 的分工

| 插件 | 取数 | 仓副本 | 发布 | 报告 |
|---|---|---|---|---|
| jira-fix | Jira | —（无） | —（Jira 不托管 git） | Jira 回帖 |
| bugfix-flow | Gitea | gitea connector fetch | Gitea PR（gate=manual） | Gitea 回帖 |
| jira-gitlab-flow | Jira | 宿主 repo_sync（GitLab） | GitLab MR（自动） | Jira 回帖+转状态 |
| **本插件** | **Jira** | **宿主 repo_sync（GitLab / Gerrit 双臂）** | **GitLab MR / Gerrit change（自动）** | **Jira 回帖+转状态** |

## 诚实边界

- **Jira Server/DC 优先**：guest 全权构造请求（REST v2），Cloud 特有 API / OAuth 未带（要接改 guest，不改清单）。
- **Gerrit Cloud 形态未带**：Basic 认证 + `)]}'` 前缀覆盖自托管 Server/DC 主流形态；Cloud 的 base64 凭据 / 嵌套 body transitions 挂账（见域侧批报告）。
- **内网靶是产品语义**：manifest `allow-private = true`（自托管 Jira/GitLab/Gerrit 常驻企业内网）；egress allowlist 仍钉三 base_url 精确值（未配置的条目自动丢弃），DNS 钉地址（rebinding 防御）照常。
- **repo_sync 不自动注册项目**：看板项目必须先建且 directory 指向仓副本落点；否则 sync 步诚实报错停车。
- **publish 失败不停车**（on-fail=continue）：缺口进 dossier/timeline，报告照常出——MR/change 没建成时 Jira 侧拿到的是「流水线完成 + 缺口注记」，不是假成功。
- **Jira 转状态按 name/to.name 不分大小写匹配**：工作流里没有可达的目标态 → failed + 可用名单（回帖已发出）；`jira_transition_name` 留空 = 只回帖不转状态（不会瞎试 transition 报噪声）。
- **未配置即诚实报错**：三个 base_url 均无声明缺省（防假缺省 URL）——对应源未配置时各路操作报「请先在面板保存基址」，不猜 URL 不空跑。
- **confirm/resume 按钮无参**：作用于最近更新的实例；per-instance 走页面上方的确认/恢复表单。
- 面板信任级 = dashboard 登录用户；guest 命令经 `plugins.ui.invoke` 过审计链，guest 内 http-send 过帧闸（egress + 预算 + 审计）。
