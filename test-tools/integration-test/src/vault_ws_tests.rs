//! Vault WSAPI 表面契约套件（L7c，2026-10-13）。
//!
//! 单节点范围：四操作（status/set/list/remove）+ 解锁测试（unlock）走真
//! gateway WS 链路，断言值永不回传与 force 轮换门。跨平台解锁语义（argon2
//! 显式口令/错误口令/锁态不残留）由 handlers/vault/tests.rs 强制 argon2id
//! fixture 覆盖——本套件跑在 Windows runner（runner 为 Windows 形态），网关
//! 默认 dpapi（open 即解锁，口令不参与），因此 unlock 不带口令即绿。
//!
//! 运行位置：main.rs 排尾（board 套件之后、cleanup 之前）——写工作区
//! `config/vault.enc`，套件结束删除恢复原状。

use serde_json::json;
use std::path::Path;
use test_harness::*;

use crate::ui_batch_series::WsApi;

const SECRET: &str = "sk-IT-L7C-SURFACE-SECRET-do-not-echo";
const ALIAS: &str = "it/vault-surface-key";

pub async fn test_vault_surface(ws: &TestWorkspace, _bin: &Path) -> Vec<TestResult> {
    let suite = "vault_ws/vault_surface";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // 1. 缺失态诚实上报：exists:false，不创建。
    let (dat, err) = api.call("vault", "status", None).await;
    match (&dat, &err) {
        (Some(d), None) if d["exists"] == false => {
            results.push(pass(&format!("{suite}/status_missing"), "exists=false"));
        }
        (Some(d), None) => results.push(fail(
            &format!("{suite}/status_missing"),
            format!("意外状态: {d}"),
        )),
        _ => results.push(fail(&format!("{suite}/status_missing"), "请求失败")),
    }

    // 2. set 创建（首写 rotated=false）。
    let (dat, err) = api
        .call(
            "vault",
            "set",
            Some(json!({ "alias": ALIAS, "value": SECRET, "domain": "it", "description": "表面契约" })),
        )
        .await;
    match (&dat, &err) {
        (Some(d), None) if d["rotated"] == false && d["alias"] == ALIAS => {
            results.push(pass(&format!("{suite}/set_creates"), "首写成功"));
        }
        _ => results.push(fail(
            &format!("{suite}/set_creates"),
            format!("意外: {dat:?} {err:?}"),
        )),
    }

    // 3. 覆盖需 force：不带 force 拒绝，文案指明 force:true。
    let (_, err) = api
        .call(
            "vault",
            "set",
            Some(json!({ "alias": ALIAS, "value": "v2" })),
        )
        .await;
    match err {
        Some(e) if e.contains("force:true") => {
            results.push(pass(
                &format!("{suite}/overwrite_needs_force"),
                "拒绝文案带指引",
            ));
        }
        Some(e) => results.push(fail(&format!("{suite}/overwrite_needs_force"), e)),
        None => results.push(fail(
            &format!("{suite}/overwrite_needs_force"),
            "未拒绝覆盖（应要求 force:true）",
        )),
    }

    // 4. force 轮换成功。
    let (dat, err) = api
        .call(
            "vault",
            "set",
            Some(json!({ "alias": ALIAS, "value": "v2", "force": true })),
        )
        .await;
    match (&dat, &err) {
        (Some(d), None) if d["rotated"] == true => {
            results.push(pass(&format!("{suite}/force_rotates"), "轮换成功"));
        }
        _ => results.push(fail(
            &format!("{suite}/force_rotates"),
            format!("意外: {dat:?} {err:?}"),
        )),
    }

    // 5. list：条目在、值永不回传。
    let (dat, err) = api.call("vault", "list", None).await;
    match &dat {
        Some(d) if err.is_none() => {
            let entries = d["entries"].as_array().cloned().unwrap_or_default();
            let dumped = serde_json::to_string(d).unwrap_or_default();
            if entries.len() == 1 && entries[0]["alias"] == ALIAS && !dumped.contains(SECRET) {
                results.push(pass(&format!("{suite}/list_surface"), "条目在、值不回传"));
            } else {
                results.push(fail(
                    &format!("{suite}/list_surface"),
                    format!("entries={entries:?} 泄漏={}", dumped.contains(SECRET)),
                ));
            }
        }
        _ => results.push(fail(&format!("{suite}/list_surface"), format!("{err:?}"))),
    }

    // 6. status：计数与模式（Windows dpapi open 即解锁 → unlocked=true）。
    let (dat, err) = api.call("vault", "status", None).await;
    match &dat {
        Some(d) if err.is_none() => {
            let ok = d["exists"] == true
                && d["alias_count"] == 1
                && d["mode"].as_str().map(|m| !m.is_empty()).unwrap_or(false)
                && d["unlocked"] == true;
            if ok {
                results.push(pass(&format!("{suite}/status_surface"), "计数/模式/解锁态"));
            } else {
                results.push(fail(&format!("{suite}/status_surface"), format!("{d}")));
            }
        }
        _ => results.push(fail(&format!("{suite}/status_surface"), format!("{err:?}"))),
    }

    // 7. 解锁测试（dpapi：open 即解锁，不带口令即绿；argon2 显式口令路径
    //    由 handler 单测覆盖）。
    let (dat, err) = api.call("vault", "unlock", None).await;
    match (&dat, &err) {
        (Some(d), None) if d["unlocked"] == true => {
            results.push(pass(&format!("{suite}/unlock"), "解锁测试通过"));
        }
        _ => results.push(fail(
            &format!("{suite}/unlock"),
            format!("意外: {dat:?} {err:?}"),
        )),
    }

    // 8. remove：未知别名诚实报错 → 删除成功 → 列表清空。
    let (_, err) = api
        .call("vault", "remove", Some(json!({ "alias": "ghost" })))
        .await;
    match err {
        Some(e) if e.contains("别名不存在") => {
            results.push(pass(&format!("{suite}/remove_unknown"), "诚实报错"));
        }
        Some(e) => results.push(fail(&format!("{suite}/remove_unknown"), e)),
        None => results.push(fail(&format!("{suite}/remove_unknown"), "未报错")),
    }
    let (dat, err) = api
        .call("vault", "remove", Some(json!({ "alias": ALIAS })))
        .await;
    match (&dat, &err) {
        (Some(d), None) if d["removed"] == true => {
            results.push(pass(&format!("{suite}/remove"), "删除成功"));
        }
        _ => results.push(fail(
            &format!("{suite}/remove"),
            format!("意外: {dat:?} {err:?}"),
        )),
    }

    // 9. 清理：删 vault 文件恢复工作区原状（套件排尾纪律——不留共享状态）。
    let vault_file = ws.workspace().join("config").join("vault.enc");
    if vault_file.exists()
        && let Err(e) = std::fs::remove_file(&vault_file)
    {
        results.push(fail(
            &format!("{suite}/cleanup"),
            format!("删除 vault 文件失败: {e}（{}）", vault_file.display()),
        ));
    }

    results
}
