# jira-gitlab-flow 域插件

WASM 插件**自持形态**（两件套：plugin.toml + guest；照 jira-fix/bugfix-flow 样板工程）。全链：**Jira（Server/DC，PAT Bearer）取 BUG → GitLab（自托管，REST v4，PRIVATE-TOKEN）仓副本 clone-or-pull → 看板落项目修复循环 → 推交付分支建 MR → Jira 回帖 + 幂等转状态**。域知识全在模板声明 + guest（connector.toml 退役形态）；宿主只提供通用原语（board_land / agent_fixloop / check_regress / repo_sync / publish / report）与 token 插值。

同一组件双导出（组合 world `plugin-panel-flow-step`）：面板命令通道（`sync` / `jira_get` / `save_settings`）+ flow 步骤执行体（`fetch` / `comment` / `report` / `create_mr` / `mr_list`）。

## 两件套

| 文件 | 角色 | 说明 |
|---|---|---|
| `plugin.toml` | 声明面 | 页面骨架（表/时间线/详情/三表单/动作钮）+ flow 剧本（七步）+ origin/回写/publish/repo_sync 声明 + egress/x-secret + guest 载荷哈希 |
| `guest/` | 血肉面 | 组合 world guest（wasm32-wasip2）：Jira/GitLab REST 双直连（http-send 过帧闸：egress + 预算 + 审计；secret-get 现取现用） |

## 剧本（jira-gitlab-pipeline，七步）

```
plugin:fetch → repo_sync → board_land → agent_fixloop(gate=manual) → check_regress → publish(自动) → report
  Jira 检索    仓副本同步     落看板项目     修复循环（唯一人工闸）        验收            建MR+缺口对账     回帖+转状态
```

- **fetch（guest 直连）**：Jira search（JQL 内联 `project = {{config.jira_project_key}}`）；`key_field`/`title_field`/`body_field` 全 `$.` JSON path（Jira search 包裹对象 `{issues:[...]}`：key 顶层、正文藏 fields 下）。`[CHECK]` 行验收锚约定照旧（写在 Jira description 里）。
- **sync（宿主 repo_sync 原语）**：项目目录 clone-or-pull（目录 = 看板项目注册表 `directory` 字段唯一真相源，**不自动注册**——先建项目再发车）；URL = `{{config.gitlab_base_url}}/{{config.gitlab_owner}}/{{config.gitlab_repo}}.git`；凭据 = vault `plugin/jira-gitlab-flow/pat`（与 publish 推送同源；GitLab HTTPS+PAT 任意非空用户名，缺省 `oauth2`）。失败 = 停车（没法修复）。
- **land → fixloop**：落看板项目 + 修复循环。**人工闸只有这一道**（gate=manual）；修复失败停车。
- **publish（自动不停）**：推交付分支 `nemesis/task-<task_id>` + GitLab MR 创建（`create_mr`）+ 缺口对账三态（`mr_list` 按 `$.source_branch` 匹配回读；Found/Absent/Inconclusive）。**无 gate**——修复内容人审，发布动作自动。`on-fail=continue`：发布失败 = 缺口记账不停车，报告照常出。
- **report（guest report op）**：Jira **幂等三步**——①GET status == 目标（`jira_transition_name`，不分大小写）→ 已完成跳过；②GET comments 同文去重 → POST 回帖（报告路径 + 看板单号）；③GET transitions 按 name/to.name 匹配 → POST 转状态。转失败 = failed 注明「回帖已发出」+ 可用 transition 名单（诚实失败，不静默）。
- **on-transition**：board 状态转移（in_review/done/cancelled）→ guest `comment` op 回帖 Jira；`origin_meta` 优先，`parse-origin-id`（`segment '/' 2`）兜旧单（origin-id 形态 `jira/<project_key>/<issue_key>`）。

## 自持写通道（实例设置表单）

面板「实例设置」表单（八 config 字段 + 双 PAT secret 字段）→ guest `save_settings` 命令：

- 8 个 config：`jira_base_url`（无缺省）/ `jira_project_key` / `jira_transition_name` / `gitlab_base_url`（无缺省）/ `gitlab_owner` / `gitlab_repo` / `gitlab_target_branch` / `board_project`。
- 双 PAT：`jira_pat`（Jira Bearer）+ `pat`（GitLab PRIVATE-TOKEN **兼 git 推送密码**——名字被宿主 publish 凭据推导钉死 `plugin/jira-gitlab-flow/pat`）。只写不读，保存后无法回显。
- 全可留空 = 保持原值；全空提交 = 页面挂载自动探活，诚实无写入应答。
- **base_url 未配置 = 模板不注册**（装载期 `{{config.*}}` 渲染失败 → WARN 跳过 = 诚实不发车，不猜 URL）——面板保存后热生效（template_stamp 重扫）。

## 安装（开发机）

```bash
# 1. 构建 guest（wasm32-wasip2 target 需先 rustup target add；
#    ⚠ guest Cargo.toml 的 [profile.release] 必须保持缺省——opt-level="z"/lto
#    会破坏 SDK wire 模块的段裁除，链接期 world 污染，见 Cargo.toml 注释）
cd guest && cargo build --target wasm32-wasip2 --release
cp target/wasm32-wasip2/release/jira_gitlab_flow_plugin.wasm ../plugin.wasm
# 2. 哈希回填 plugin.toml 的 wasm-sha256
sha256sum ../plugin.wasm
# 3. 装配（--allow-unsigned 自担信任；生产先 plugin keygen/sign/trust）
nemesisbot plugin install plugins/wasm/jira-gitlab-flow --yes --allow-unsigned
nemesisbot plugin enable jira-gitlab-flow    # 新装默认不加载（安装≠加载）
# 4. 重启 gateway → Dashboard 侧栏【定制】分组出现「Jira+GitLab 修复流水线」
# 5. 面板「实例设置」保存双基址 + 双 PAT；看板侧注册项目（与 board_project
#    配置同名、directory 指向仓副本落点）→ 按钮触发首次取数或等 interval
```

## 换靶坐标

| 要改什么 | 改哪里 |
|---|---|
| Jira/GitLab 实例地址 | 面板「实例设置」（config 双 base_url；egress 随之热生效） |
| 目标项目 / 仓库坐标 | 面板「实例设置」（`jira_project_key` / `gitlab_owner` / `gitlab_repo`） |
| MR 目标分支 / 完成态 | 面板「实例设置」（`gitlab_target_branch` / `jira_transition_name`） |
| 导入范围 | `plugin.toml` fetch 步 JQL（当前 `status not in (Done, Closed)`；换靶改 `jira_project_key` 即换 JQL——单遍渲染不支持跨键引用，JQL 不做 config 键） |
| 看板项目绑定 | 面板「实例设置」（`board_project`；须与看板注册项目同名且带 directory） |
| 回写/报告文案 | `on-transition.args.body` / publish `create_args.description` / report `args_json` 模板 |

## 与 jira-fix / bugfix-flow 的分工

| 插件 | 取数 | 仓副本 | 发布 | 报告 |
|---|---|---|---|---|
| jira-fix | Jira | —（无） | —（Jira 不托管 git） | Jira 回帖 |
| bugfix-flow | Gitea | gitea connector fetch | Gitea PR（gate=manual） | Gitea 回帖 |
| **本插件** | **Jira** | **宿主 repo_sync（GitLab）** | **GitLab MR（自动）** | **Jira 回帖+转状态** |

Jira 管工单、GitLab 管代码的团队用本插件单件全链；仓库不在 GitLab 时换 bugfix-flow 组合 jira-fix（看板单是共同执行真相源）。

## 诚实边界

- **Jira Server/DC PAT Bearer**：guest 全权构造请求，Cloud Basic / OAuth 未带（要接改 guest，不改清单）。GitLab 侧 PRIVATE-TOKEN（PAT）。
- **内网靶是产品语义**：manifest `allow-private = true`（自托管 Jira/GitLab 常驻企业内网）；egress allowlist 仍钉双 base_url 精确值，DNS 钉地址（rebinding 防御）照常。
- **repo_sync 不自动注册项目**：看板项目必须先建（dashboard 项目区或 `projects.create`）且 directory 指向仓副本落点；否则 sync 步诚实报错停车。
- **publish 失败不停车**（on-fail=continue）：缺口进 dossier/timeline，报告照常出——但 MR 没建成时 Jira 侧拿到的是「流水线完成 + 缺口注记」，不是假成功。
- **Jira 转状态按 name/to.name 不分大小写匹配**：工作流里没有可达的目标态 → failed + 可用名单（回帖已发出）；中间态人工转或调 `jira_transition_name`。
- **confirm/resume 按钮无参**：作用于最近更新的实例；per-instance 走页面上方的确认/恢复表单。
- 面板信任级 = dashboard 登录用户；guest 命令经 `plugins.ui.invoke` 过审计链，guest 内 http-send 过帧闸（egress + 预算 + 审计）。
