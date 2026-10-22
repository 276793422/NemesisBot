//! guest 导出调用（instantiate + 按名分发 + 结果 JSON 化）。
//!
//! 调用面导出名：`get-metadata` / `execute`（tool world）、`observe`
//! （observer world）、`get-policy-metadata` / `compact-check` /
//! `judge-final` / `validate-args`（loop-policy world，2026-10-22 批 3）。
//! 每调用 fresh store + fresh linker + 一次实例化
//!（fresh-store-per-call 的宿主侧对齐形态）。

use std::sync::Arc;

use wasmtime::component::{Component, Val};
use wasmtime::Store;

use crate::abi::status;
use crate::store::{fresh_linker, ShellCtx};

/// tool world 导出接口全名。
const TOOL_IFACE: &str = "nemesis:plugin/tool@0.1.0";
/// observer world 导出接口全名。
const OBSERVER_IFACE: &str = "nemesis:plugin/observer@0.1.0";
/// loop-policy world 导出接口全名（2026-10-22 批 3）。
const POLICY_IFACE: &str = "nemesis:plugin/loop-policy@0.1.0";

/// world 归属（组件只导出自己 world 的接口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorldKind {
    Tool,
    Observer,
    Policy,
}

/// 编译产物 + world 导出索引（component_new 期一次性解析）。
pub(crate) struct CompiledComponent {
    pub(crate) component: Arc<Component>,
    pub(crate) world: WorldKind,
    tool_meta: Option<wasmtime::component::ComponentExportIndex>,
    tool_execute: Option<wasmtime::component::ComponentExportIndex>,
    obs_observe: Option<wasmtime::component::ComponentExportIndex>,
    pol_meta: Option<wasmtime::component::ComponentExportIndex>,
    pol_compact: Option<wasmtime::component::ComponentExportIndex>,
    pol_judge: Option<wasmtime::component::ComponentExportIndex>,
    pol_validate: Option<wasmtime::component::ComponentExportIndex>,
}

/// 编译 + world 识别。都不是已知 world = 诚实失败（非本合同组件拒收）。
pub(crate) fn compile(engine: &wasmtime::Engine, wasm: &[u8]) -> Result<CompiledComponent, String> {
    let c = Component::new(engine, wasm).map_err(|e| format!("compile: {e}"))?;
    let tool_iface = c.get_export_index(None, TOOL_IFACE);
    let obs_iface = c.get_export_index(None, OBSERVER_IFACE);
    let pol_iface = c.get_export_index(None, POLICY_IFACE);
    let (
        world,
        tool_meta,
        tool_execute,
        obs_observe,
        pol_meta,
        pol_compact,
        pol_judge,
        pol_validate,
    ) = if let Some(iface) = tool_iface {
        let meta = c
            .get_export_index(Some(&iface), "get-metadata")
            .ok_or("tool world missing get-metadata export")?;
        let exec = c
            .get_export_index(Some(&iface), "execute")
            .ok_or("tool world missing execute export")?;
        (
            WorldKind::Tool,
            Some(meta),
            Some(exec),
            None,
            None,
            None,
            None,
            None,
        )
    } else if let Some(iface) = obs_iface {
        let obs = c
            .get_export_index(Some(&iface), "observe")
            .ok_or("observer world missing observe export")?;
        (
            WorldKind::Observer,
            None,
            None,
            Some(obs),
            None,
            None,
            None,
            None,
        )
    } else if let Some(iface) = pol_iface {
        let meta = c
            .get_export_index(Some(&iface), "get-policy-metadata")
            .ok_or("policy world missing get-policy-metadata export")?;
        let compact = c
            .get_export_index(Some(&iface), "compact-check")
            .ok_or("policy world missing compact-check export")?;
        let judge = c
            .get_export_index(Some(&iface), "judge-final")
            .ok_or("policy world missing judge-final export")?;
        let validate = c
            .get_export_index(Some(&iface), "validate-args")
            .ok_or("policy world missing validate-args export")?;
        (
            WorldKind::Policy,
            None,
            None,
            None,
            Some(meta),
            Some(compact),
            Some(judge),
            Some(validate),
        )
    } else {
        return Err(
            "component exports neither plugin-tool, plugin-observer nor plugin-loop-policy world"
                .into(),
        );
    };
    Ok(CompiledComponent {
        component: Arc::new(c),
        world,
        tool_meta,
        tool_execute,
        obs_observe,
        pol_meta,
        pol_compact,
        pol_judge,
        pol_validate,
    })
}

/// 调用失败 = (状态码, last_error 消息)。
pub(crate) type CallFailure = (i32, String);

/// 执行一次导出调用，返回结果 JSON（status=ok 时有效）。
pub(crate) fn call_export(
    store: &mut Store<ShellCtx>,
    comp: &CompiledComponent,
    name: &str,
    args_json: &str,
) -> Result<String, CallFailure> {
    let engine = store.engine().clone();
    let linker = fresh_linker(&engine);
    let instance = linker
        .instantiate(&mut *store, &comp.component)
        .map_err(|e| (status::INSTANTIATE, trunc(&e.to_string())))?;

    match (name, comp.world) {
        ("get-metadata", WorldKind::Tool) => {
            let idx = comp.tool_meta.expect("validated at compile");
            let func = instance
                .get_func(&mut *store, idx)
                .ok_or((status::INTERNAL, "get-metadata func missing".into()))?;
            let mut results = val_results(&func, &mut *store, 1);
            func.call(&mut *store, &[], &mut results)
                .map_err(classify_call)?;
            unwrap_result_val(&results[0])
        }
        ("execute", WorldKind::Tool) => {
            let idx = comp.tool_execute.expect("validated at compile");
            let input = tool_input_val(args_json).map_err(|e| (status::BAD_ARGS, e))?;
            let func = instance
                .get_func(&mut *store, idx)
                .ok_or((status::INTERNAL, "execute func missing".into()))?;
            let mut results = val_results(&func, &mut *store, 1);
            func.call(&mut *store, &[input], &mut results)
                .map_err(classify_call)?;
            unwrap_result_val(&results[0])
        }
        ("observe", WorldKind::Observer) => {
            let idx = comp.obs_observe.expect("validated at compile");
            let func = instance
                .get_func(&mut *store, idx)
                .ok_or((status::INTERNAL, "observe func missing".into()))?;
            let mut results = val_results(&func, &mut *store, 1);
            func.call(
                &mut *store,
                &[Val::String(args_json.to_string())],
                &mut results,
            )
            .map_err(classify_call)?;
            unwrap_result_val(&results[0])
        }
        ("get-policy-metadata", WorldKind::Policy) => {
            let idx = comp.pol_meta.expect("validated at compile");
            let func = instance
                .get_func(&mut *store, idx)
                .ok_or((status::INTERNAL, "get-policy-metadata func missing".into()))?;
            let mut results = val_results(&func, &mut *store, 1);
            func.call(&mut *store, &[], &mut results)
                .map_err(classify_call)?;
            unwrap_result_val(&results[0])
        }
        // 三策略点同形：单 json-string 进、result<json-string, host-error> 出。
        ("compact-check" | "judge-final" | "validate-args", WorldKind::Policy) => {
            let idx = match name {
                "compact-check" => comp.pol_compact.expect("validated at compile"),
                "judge-final" => comp.pol_judge.expect("validated at compile"),
                _ => comp.pol_validate.expect("validated at compile"),
            };
            let func = instance
                .get_func(&mut *store, idx)
                .ok_or((status::INTERNAL, format!("{name} func missing")))?;
            let mut results = val_results(&func, &mut *store, 1);
            func.call(
                &mut *store,
                &[Val::String(args_json.to_string())],
                &mut results,
            )
            .map_err(classify_call)?;
            unwrap_result_val(&results[0])
        }
        // world 误配（拿别的 world 的导出名调用）诚实报错。
        ("get-metadata" | "execute", WorldKind::Observer | WorldKind::Policy) => Err((
            status::BAD_ARGS,
            "component is not a tool plugin (no tool exports)".into(),
        )),
        ("observe", WorldKind::Tool | WorldKind::Policy) => Err((
            status::BAD_ARGS,
            "component is not an observer plugin (no observe export)".into(),
        )),
        (
            "get-policy-metadata" | "compact-check" | "judge-final" | "validate-args",
            WorldKind::Tool | WorldKind::Observer,
        ) => Err((
            status::BAD_ARGS,
            "component is not a loop-policy plugin (no policy exports)".into(),
        )),
        (other, _) => Err((status::BAD_ARGS, format!("unknown export name: {other}"))),
    }
}

/// tool-input record 构造（合同静态形状；宿主 exe 传入完整 JSON）。
fn tool_input_val(args_json: &str) -> Result<Val, String> {
    let v: serde_json::Value =
        serde_json::from_str(args_json).map_err(|e| format!("tool-input json: {e}"))?;
    let args = v
        .get("args-json")
        .and_then(serde_json::Value::as_str)
        .ok_or("tool-input missing args-json")?;
    let ctx = v
        .get("context")
        .ok_or("tool-input missing context")?
        .as_object()
        .ok_or("tool-input context must be object")?;
    let get = |k: &str| -> Result<String, String> {
        ctx.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or(format!("tool-input context missing {k}"))
    };
    Ok(Val::Record(vec![
        ("args-json".into(), Val::String(args.to_string())),
        (
            "context".into(),
            Val::Record(vec![
                ("workspace-root".into(), Val::String(get("workspace-root")?)),
                ("session-key".into(), Val::String(get("session-key")?)),
                ("call-id".into(), Val::String(get("call-id")?)),
            ]),
        ),
    ]))
}

/// 预填结果槽（dynamic API：调用方提供等长结果数组，call 时被覆写）。
/// 出参个数从函数类型读取（wasmtime 48 无 Func::results，经 ty() 取）。
fn val_results(
    func: &wasmtime::component::Func,
    mut store: impl wasmtime::AsContextMut,
    expect: usize,
) -> Vec<Val> {
    let n = func.ty(&mut store).results().len();
    debug_assert_eq!(n, expect, "contract export arity");
    vec![Val::Bool(false); n]
}

/// result<T, host-error> Val → 结果 JSON。
///
/// ok 臂 → `{"ok": <payload>}`；err 臂（host-error 变体）→
/// `{"err": {"tag": …, "detail": …}}`。
fn unwrap_result_val(v: &Val) -> Result<String, CallFailure> {
    let Val::Result(arm) = v else {
        return Err((
            status::INTERNAL,
            format!("unexpected export result shape: {v:?}"),
        ));
    };
    match arm {
        Ok(payload) => {
            let json = payload
                .as_ref()
                .map(|b| crate::store::val_to_json_value(b))
                .unwrap_or(serde_json::Value::Null);
            Ok(serde_json::json!({ "ok": json }).to_string())
        }
        Err(Some(inner)) => {
            let (tag, detail) = match inner.as_ref() {
                Val::Variant(name, Some(p)) => {
                    (name.clone(), crate::store::val_to_json_value(p).to_string())
                }
                Val::Variant(name, None) => (name.clone(), String::new()),
                other => ("unavailable".into(), format!("{other:?}")),
            };
            Ok(serde_json::json!({ "err": { "tag": tag, "detail": detail } }).to_string())
        }
        Err(None) => {
            Ok(serde_json::json!({ "err": { "tag": "unavailable", "detail": "" } }).to_string())
        }
    }
}

/// wasmtime 调用错误归类：fuel/epoch 预算 → TIMEOUT，其余 → TRAP。
/// （对齐宿主旧 classify_trap + is_epoch_timeout；downcast 能力在本壳，
/// 宿主 exe 不再持有 wasmtime 类型。）
fn classify_call(e: wasmtime::Error) -> CallFailure {
    if is_budget_exhausted(&e) {
        (
            status::TIMEOUT,
            "fuel or wall-clock budget exhausted".into(),
        )
    } else {
        (status::TRAP, trunc(&error_chain(&e)))
    }
}

/// 错误全链文本（Display + Causes；wasm trap 的真实原因在 cause 链里，
/// 顶层 Display 只有 backtrace）。
fn error_chain(e: &wasmtime::Error) -> String {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(e.as_ref() as &(dyn std::error::Error + 'static));
    while let Some(err) = source {
        msg.push_str(" :: ");
        msg.push_str(&err.to_string());
        source = err.source();
    }
    msg
}

/// epoch 中断 / fuel 耗尽判定（沿 source 链 downcast Trap + 文本兜底）。
fn is_budget_exhausted(e: &wasmtime::Error) -> bool {
    let mut source: Option<&dyn std::error::Error> = Some(e.as_ref());
    while let Some(err) = source {
        if let Some(trap) = err.downcast_ref::<wasmtime::Trap>() {
            if matches!(trap, wasmtime::Trap::Interrupt | wasmtime::Trap::OutOfFuel) {
                return true;
            }
        }
        source = err.source();
    }
    let text = e.to_string().to_lowercase();
    text.contains("all fuel consumed")
        || text.contains("fuel exhausted")
        || text.contains("epoch deadline")
}

/// 错误文本截断（对齐宿主 truncate_err 500 上限，字符边界安全）。
fn trunc(s: &str) -> String {
    if s.chars().count() <= 500 {
        return s.to_string();
    }
    s.chars().take(500).collect()
}
