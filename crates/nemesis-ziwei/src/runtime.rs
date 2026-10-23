//! runtime —— M4 运行期调用统一实现（call_fn / hook_capture）：agent 子
//! 工具桥（nemesisbot `ziwei_tools`）与 WSAPI `ziwei.invoke` / `ziwei.resolve`
//! 同源消费——单一真相源，两入口语义恒一致。
//!
//! 入参纪律（T4.1）：call_fn 只收整数位置参数 ≤4（浮点/字符串/结构指针
//! loud 拒绝；负数按补码转 u64——指针宽度语义）；hook_capture 全锚幂等
//! 订阅 + 拉取解码回流 + `output.dest` 报表渲染（T4.5）；链长超限运行期
//! 防御再拦（T4.6）。

// invoke_call_fn / invoke_hook_capture / load_fn_def 全部 windows-only
// （会话活体路径）；平台中立面只有 callfn 参数解析两函数。
#[cfg(windows)]
use std::sync::Arc;

#[cfg(windows)]
use crate::profile::{CHAIN_CAP, FunctionDef, Profile, ProfileStore};
#[cfg(windows)]
use crate::session::SessionManager;

/// 整数参数组解析核心（`serde_json::Value` 切片形态；WSAPI 直用）。
pub fn callfn_args_from_slice(items: &[serde_json::Value]) -> Result<[u64; 4], String> {
    if items.len() > 4 {
        return Err("args 最多 4 个（x64 快四寄存器 / x86 栈四槽）".to_string());
    }
    let mut out = [0u64; 4];
    for (i, item) in items.iter().enumerate() {
        let n = item.as_i64().ok_or_else(|| {
            format!("args[{i}] 非整数（v1 不接受浮点/字符串/结构指针；浮点参须走后续 binary 面）")
        })?;
        out[i] = n as u64;
    }
    Ok(out)
}

/// 工具入参 JSON → 整数参（agent 桥形态 `{"args":[..]}`；缺 args = 全零）。
pub fn parse_callfn_args(raw: &str) -> Result<[u64; 4], String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("参数非合法 JSON：{e}"))?;
    match v.get("args") {
        None => Ok([0; 4]),
        Some(a) => callfn_args_from_slice(
            a.as_array()
                .ok_or("args 须为整数数组（call_fn v1 只接受整数位置参数）")?,
        ),
    }
}

/// 加载 profile 并取函数定义（call_fn / hook_capture 公共前置）。
#[cfg(windows)]
fn load_fn_def(
    workspace: &std::path::Path,
    profile_id: &str,
    semantic: &str,
) -> Result<(Profile, FunctionDef), String> {
    let profile = ProfileStore::new(workspace)
        .load(profile_id)
        .map_err(|e| e.to_string())?;
    let fdef = profile
        .functions
        .iter()
        .find(|f| f.semantic == semantic)
        .ok_or_else(|| format!("函数 {semantic} 在 profile 中未定义"))?
        .clone();
    Ok((profile, fdef))
}

/// call_fn 统一执行（T4.1）：语义名 → 地址（空表自动 resolve）→ conv 取
/// profile FunctionDef → 整数参 → 会话 call_fn_addr（Busy/看门狗预算在会话层）。
/// `timeout_ms` 由调用方从 ToolAction 取（缺省 5000，上限 60000）。
#[cfg(windows)]
pub async fn invoke_call_fn(
    manager: &Arc<SessionManager>,
    workspace: &std::path::Path,
    profile_id: &str,
    semantic: &str,
    args: [u64; 4],
    timeout_ms: u32,
) -> Result<serde_json::Value, String> {
    let session = manager.get(profile_id).ok_or_else(|| {
        format!("会话未附加（profile {profile_id}）：先经 ziwei.attach 附加目标进程")
    })?;
    let (_profile, fdef) = load_fn_def(workspace, profile_id, semantic)?;
    // 地址：查表；未解析（含空表）先跑一遍 resolve 引擎
    let addr = match session.fn_addr(semantic) {
        Some(a) => a,
        None => {
            manager.resolve_functions(profile_id).await?;
            session.fn_addr(semantic).ok_or_else(|| {
                format!("函数 {semantic} 地址解析失败（锚未命中，见 ziwei.resolve notes）")
            })?
        }
    };
    let conv = crate::resolve::conv_to_code(fdef.conv);
    let mut obj = manager
        .call_fn_addr(profile_id, addr, conv, args, timeout_ms)
        .await?;
    if let Some(o) = obj.as_object_mut() {
        o.insert("fn".into(), serde_json::json!(semantic));
    }
    Ok(obj)
}

/// hook_capture 统一执行（T4.3/T4.5/T4.6）：全锚幂等订阅（函数 + 链步骤）
/// → 拉取解码回流 → `output.dest` 报表渲染（输出对象含 tool 无关面——
/// 工具身份字段由调用方补）。
#[cfg(windows)]
pub async fn invoke_hook_capture(
    manager: &Arc<SessionManager>,
    workspace: &std::path::Path,
    profile_id: &str,
    semantic: &str,
    output_dest: Option<&str>,
    max_records: u32,
) -> Result<serde_json::Value, String> {
    let session = manager.get(profile_id).ok_or_else(|| {
        format!("会话未附加（profile {profile_id}）：先经 ziwei.attach 附加目标进程")
    })?;
    let (_profile, fdef) = load_fn_def(workspace, profile_id, semantic)?;
    // T4.6 运行期防御再拦（profile validate 已强制，此处兜底）
    let chain_len = fdef.capture.as_ref().map(|c| c.chain.len()).unwrap_or(0);
    if chain_len > CHAIN_CAP {
        return Err(format!("捕获链长 {chain_len} 超上限（{CHAIN_CAP}）"));
    }
    // 地址未解析（空表）先跑一遍 resolve 引擎
    if session.fn_keys().is_empty() {
        manager.resolve_functions(profile_id).await?;
    }
    // 订阅全部已解析锚（函数 + 链步骤），幂等；未命中锚诚实跳过
    let mut anchors = vec![semantic.to_string()];
    for i in 0..chain_len {
        anchors.push(format!("{semantic}::chain{i}"));
    }
    let mut hooked = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    for key in &anchors {
        if session.fn_addr(key).is_none() {
            skipped.push(key.clone());
            continue;
        }
        manager.hook_start(profile_id, key).await?;
        hooked += 1;
    }
    // 拉取 + 解码（regs[0] 指针）+ capture.jsonl 回流
    let pull = manager.poll_captures(profile_id).await?;
    let mut obj = serde_json::json!({
        "fn": semantic,
        "hooks_active": hooked,
        "anchors_skipped": skipped,
        "count": pull["count"].clone(),
        "stored": pull["stored"].clone(),
        "dropped": pull["dropped"].clone(),
        "events": pull["events"].clone(),
    });
    // T4.5：output.dest 渲染（workspace/reports/<dest>；jsonl 形态）
    if let Some(dest) = output_dest {
        let records = manager
            .capture()
            .read_recent(profile_id, max_records as usize);
        let report_path = workspace.join("reports").join(dest);
        if let Some(parent) = report_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("报表目录创建失败（{}）：{e}", parent.display()))?;
        }
        let mut buf = String::new();
        for r in &records {
            buf.push_str(&serde_json::to_string(r).unwrap_or_default());
            buf.push('\n');
        }
        std::fs::write(&report_path, buf)
            .map_err(|e| format!("报表写入失败（{}）：{e}", report_path.display()))?;
        if let Some(o) = obj.as_object_mut() {
            o.insert(
                "report".into(),
                serde_json::json!(report_path.display().to_string()),
            );
            o.insert("report_records".into(), serde_json::json!(records.len()));
        }
    }
    Ok(obj)
}

#[cfg(test)]
mod tests;
