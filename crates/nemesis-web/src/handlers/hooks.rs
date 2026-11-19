//! Hooks handler — (2026-08-24 UI entry gap) 设置页「Hooks」Tab 后端。
//! （2026-08-29：路径收编至 `<workspace>/config/hooks.json`，legacy 一次性
//! copy-once 迁移——见 cc_hooks::migrate_legacy_home_hooks_config。）
//!
//! `<workspace>/config/hooks.json` 是 hooks.json 方言（`nemesis_agent::cc_hooks`）：
//! 十二事件（PascalCase），每条 hook 二型——command 型 = 子进程脚本（stdin
//! JSON / env / 退出码拦放行，任意事件）；prompt 型 = 提示词注入（LLM 执行
//! 体，仅 Stop——turn 收尾后开新轮）。统一存储（2026-11-19）：会话钩子（agent
//! `hooks` 工具写入）同住本文件的 scope 圈定组——`set` 写前持
//! [`cc_hooks::lock_hooks_file`] 互斥，避免与工具 RMW 竞态。
//!
//! - `get`：读文件；**不存在 → 返回空模板而非错误**（fresh home 常态）。
//!   文件存在但解析失败也照样返回原文 + `valid:false` + 错误详情 —— 用户
//!   要在编辑器里修好它，报错不给出原文等于让人盲修。
//! - `set`：先 `parse_cc_hooks` 语义校验（JSON 语法 + 方言格式），
//!   **校验失败不落盘**；通过则原文照写（不 pretty 重排 —— 保用户键序）。
//! - `summary`：每事件脚本数（get/set 都带，UI 显示「当前 N 个脚本」）。
//!
//! 生效机制（2026-09-24 热更收口件1；2026-11-20 即刻生效语义）：桥内分发
//! 入口惰性检查 hooks.json 的内容（内容比对不赌 mtime），变化即重新解析
//! 换装（解析失败保旧 / 清空或删除=注销）——保存后下条消息生效，无需重启
//! Agent。`CcHookBridge::load_from_dir`（agent_factory）**恒建桥**（缺失/
//! 空文件/解析失败也建桥待命）——首次保存同样即刻生效，无「重启建桥」
//! 中间态。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::cc_hooks::{self, CcEvents};

pub struct HooksHandler;

#[async_trait::async_trait]
impl ModuleHandler for HooksHandler {
    fn module_name(&self) -> &str {
        "hooks"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["get", "set"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = crate::handlers::require_workspace(ctx)?;
        let home = crate::handlers::require_home(ctx)?;
        // legacy 落位（<home>/config/hooks.json）一次性迁移到新落位
        // （<workspace>/config/hooks.json）——copy-once，幂等。
        nemesis_agent::cc_hooks::migrate_legacy_home_hooks_config(
            std::path::Path::new(&home).join("config").as_path(),
            &nemesis_path::workspace_config_dir(std::path::Path::new(&workspace)),
        );
        match cmd {
            "get" => self.get(workspace),
            "set" => {
                let data = data.ok_or("missing data")?;
                let content = crate::handlers::get_str(&data, "content")?;
                self.set(workspace, &content)
            }
            _ => Err(format!("unknown command: hooks.{}", cmd)),
        }
    }
}

impl HooksHandler {
    /// 空模板：五事件全空（合法方言格式、0 脚本），既是骨架也是事件名速查。
    /// （SubagentStart/SubagentStop 不入模板——子代理钩子是进阶用法，速查
    /// 表见设置页事件区块。）
    fn empty_template() -> String {
        serde_json::to_string_pretty(&serde_json::json!({
            "hooks": {
                "PreToolUse": [],
                "PostToolUse": [],
                "SessionStart": [],
                "UserPromptSubmit": [],
                "Stop": []
            }
        }))
        .unwrap()
    }

    pub(crate) fn get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path =
            nemesis_path::resolve_hooks_config_path_in_workspace(std::path::Path::new(workspace));
        // BUG-20261119-179：走 read_config_string 剥 BOM（hooks.json 是用户
        // 可手工编辑的 JSON 配置，记事本「带 BOM 的 UTF-8」保存形态会让
        // serde_json 报无从定位的「expected value at line 1 column 1」）——
        // 与 cc_hooks 模块内部读取点同口径（本模块此前是唯一裸读臂）。
        match nemesis_utils::fsutil::read_config_string(&path) {
            Ok(content) => {
                let (valid, error, summary) = match cc_hooks::parse_cc_hooks(&content) {
                    Ok(events) => (true, serde_json::Value::Null, Some(events_summary(&events))),
                    Err(e) => (false, serde_json::Value::String(e), None),
                };
                Ok(Some(serde_json::json!({
                    "exists": true,
                    "content": content,
                    "valid": valid,
                    "error": error,
                    "summary": summary,
                })))
            }
            Err(_) => Ok(Some(serde_json::json!({
                // 没配 hooks = 常态，给可编辑的空模板（goal §七：不报错）。
                "exists": false,
                "content": Self::empty_template(),
                "valid": true,
                "error": serde_json::Value::Null,
                "summary": events_summary(&CcEvents::default()),
            }))),
        }
    }

    pub(crate) fn set(
        &self,
        workspace: &str,
        content: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        // 语义校验先于一切 IO（goal：校验失败不落盘）。校验 + 写入全程持
        // hooks.json RMW 互斥——agent `hooks` 工具与 Dashboard 并发写同一
        // 文件，读-改-写竞态靠这把闸封（bridge 侧热更只读，无竞态）。
        let _guard = cc_hooks::lock_hooks_file();
        let events = cc_hooks::parse_cc_hooks(content)?;
        let path =
            nemesis_path::resolve_hooks_config_path_in_workspace(std::path::Path::new(workspace));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create config dir: {e}"))?;
        }
        // 统一原子写入（hooks.json）。
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
            .map_err(|e| format!("failed to write hooks.json: {e}"))?;
        Ok(Some(serde_json::json!({
            "written": true,
            "summary": events_summary(&events),
        })))
    }
}

/// 每事件脚本数 + 总数（`CcEvents::script_counts` 统一定义）。
fn events_summary(events: &CcEvents) -> serde_json::Value {
    let counts = events.script_counts();
    let mut obj = serde_json::Map::new();
    let mut total = 0usize;
    for (name, n) in counts {
        obj.insert(name.to_string(), serde_json::Value::from(n));
        total += n;
    }
    obj.insert("total".to_string(), serde_json::Value::from(total));
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests;
