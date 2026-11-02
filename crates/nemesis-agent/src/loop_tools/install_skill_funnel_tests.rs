//! BUG-20261101-295：`install_skill` 工具 WS4 漏斗接线回归。
//!
//! 旧形态裸 `RegistryManager::install` 直落 skills/，绕过验签(P11)/安全
//! 扫描(P15)/审批门(P13)/lockfile 记账(P14)/staging 全漏斗——web/CLI 同
//! 操作均走 `SkillInstaller`，工具面是唯一旁路。本文件钉住：
//! - 漏斗路径生效（lockfile/origin 记账 = 走过漏斗的凭证）；
//! - P13 审批门经共享槽生效（含「建工具后晚填槽」的 gateway 时序）；
//! - slug 闸与 exists/force 语义保持。

use super::*;
use nemesis_skills::install_gate::{AlwaysDenyGate, InstallDecision, InstallPlan};
use tempfile::TempDir;

/// 捕获审批门收到的 plan.source（验证门真被咨询 + 晚填槽可见）。
struct RecordingGate {
    seen: std::sync::Mutex<Vec<String>>,
    decision: bool,
}

#[async_trait::async_trait]
impl nemesis_skills::install_gate::InstallGate for RecordingGate {
    async fn decide(&self, plan: &InstallPlan) -> InstallDecision {
        self.seen
            .lock()
            .unwrap()
            .push(format!("{}/{}", plan.source, plan.slug));
        if self.decision {
            InstallDecision::Approve
        } else {
            InstallDecision::Deny {
                reason: "test deny".to_string(),
            }
        }
    }
}

fn stub_registry() -> Arc<nemesis_skills::registry::RegistryManager> {
    let manager = nemesis_skills::registry::RegistryManager::new_empty();
    manager.add_registry(Arc::new(nemesis_skills::registry::StubRegistryProvider));
    Arc::new(manager)
}

fn temp_ws() -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    (dir, ws)
}

/// 无审批门（槽缺席）：安装成功且 lockfile/origin 记账在——漏斗路径凭证
/// （旧 RegistryManager 直装路径从不写这两样）。
#[tokio::test]
async fn funnel_install_records_lockfile_and_origin() {
    let (_dir, ws) = temp_ws();
    let tool = InstallSkillTool::new(stub_registry(), ws.clone(), None);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let result = tool
        .execute(r#"{"name": "funnel-skill", "registry": "stub"}"#, &ctx)
        .await
        .unwrap();
    assert!(result.contains("installed successfully"), "got: {}", result);

    let skill_dir = std::path::Path::new(&ws)
        .join("skills")
        .join("funnel-skill");
    assert!(skill_dir.is_dir(), "skill dir must exist after install");
    assert!(
        skill_dir.join(".skill-origin.json").exists(),
        "origin tracking must be written by the funnel"
    );
    let lockfile = std::path::Path::new(&ws).join("skills.lock.json");
    assert!(lockfile.exists(), "lockfile must be recorded by the funnel");
    let lock_text = std::fs::read_to_string(&lockfile).unwrap();
    assert!(
        lock_text.contains("funnel-skill") && lock_text.contains("registry:stub/funnel-skill"),
        "lockfile entry must carry the funnel source tag, got: {}",
        lock_text
    );
}

/// 敌意 slug 在任何文件系统副作用之前拒绝。
#[tokio::test]
async fn hostile_slug_rejected_before_fs() {
    let (_dir, ws) = temp_ws();
    let tool = InstallSkillTool::new(stub_registry(), ws.clone(), None);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    for hostile in ["", "../x", "a/b", "a\\b"] {
        let args = serde_json::json!({ "name": hostile, "registry": "stub" }).to_string();
        if hostile.is_empty() {
            // 空串在参数提取臂拒绝（required non-empty string）。
            assert!(tool.execute(&args, &ctx).await.is_err());
        } else {
            let err = tool.execute(&args, &ctx).await.unwrap_err();
            assert!(
                err.contains("invalid slug"),
                "hostile slug {:?} must fail at the slug gate, got: {}",
                hostile,
                err
            );
        }
    }
    let skills = std::path::Path::new(&ws).join("skills");
    assert!(!skills.exists(), "no fs side effects for hostile slugs");
}

/// P13 审批门拒绝臂：槽内有门时安装被拒，skills/ 与 lockfile 零残留。
#[tokio::test]
async fn gate_deny_blocks_install_and_leaves_no_trace() {
    let (_dir, ws) = temp_ws();
    let slot: SkillsInstallGateSlot =
        Arc::new(parking_lot::RwLock::new(Some(Arc::new(AlwaysDenyGate))));
    let tool = InstallSkillTool::new(stub_registry(), ws.clone(), Some(slot));
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let err = tool
        .execute(r#"{"name": "denied-skill", "registry": "stub"}"#, &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("denied by approval gate"),
        "gate deny must surface, got: {}",
        err
    );
    let skills = std::path::Path::new(&ws).join("skills");
    assert!(
        !skills.join("denied-skill").exists(),
        "no skill dir on deny"
    );
    assert!(
        !std::path::Path::new(&ws).join("skills.lock.json").exists(),
        "no lockfile entry on deny"
    );
}

/// gateway 晚填时序：工具先建（槽空）→ 填门 → 安装——门必须可见
/// （approval_slot 同款「先建槽后填」契约）。
#[tokio::test]
async fn late_filled_gate_is_visible_to_existing_tool() {
    let (_dir, ws) = temp_ws();
    let slot: SkillsInstallGateSlot = Arc::new(parking_lot::RwLock::new(None));
    let tool = InstallSkillTool::new(stub_registry(), ws.clone(), Some(slot.clone()));
    // gateway 时序：loop 已建，此刻才填门。
    *slot.write() = Some(Arc::new(RecordingGate {
        seen: std::sync::Mutex::new(Vec::new()),
        decision: false,
    }));
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let err = tool
        .execute(r#"{"name": "late-gate-skill", "registry": "stub"}"#, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("denied by approval gate"), "got: {}", err);
}

/// 晚填的 RecordingGate 收到的 plan 带漏斗构造的 source/slug。
#[tokio::test]
async fn gate_receives_funnel_plan() {
    let (_dir, ws) = temp_ws();
    let slot: SkillsInstallGateSlot = Arc::new(parking_lot::RwLock::new(None));
    let tool = InstallSkillTool::new(stub_registry(), ws.clone(), Some(slot.clone()));
    let gate = Arc::new(RecordingGate {
        seen: std::sync::Mutex::new(Vec::new()),
        decision: true,
    });
    *slot.write() = Some(gate.clone() as Arc<dyn nemesis_skills::install_gate::InstallGate>);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    tool.execute(
        r#"{"name": "plan-skill", "registry": "stub", "version": "9.9"}"#,
        &ctx,
    )
    .await
    .unwrap();
    let seen = gate.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "gate consulted exactly once");
    assert!(
        seen[0].contains("registry:stub/plan-skill"),
        "plan.source must identify registry+slug, got: {}",
        seen[0]
    );
}

/// exists 无 force 拒绝（原语义保持）；force 先卸旧再装（lockfile 感知）。
#[tokio::test]
async fn exists_and_force_semantics_preserved() {
    let (_dir, ws) = temp_ws();
    // 预置旧版（手造目录，无 lockfile 记账——uninstall 仍应能清）。
    let skill_dir = std::path::Path::new(&ws).join("skills").join("replace-me");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("OLD.md"), "old").unwrap();

    let tool = InstallSkillTool::new(stub_registry(), ws.clone(), None);
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let err = tool
        .execute(r#"{"name": "replace-me", "registry": "stub"}"#, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("already exists"), "got: {}", err);
    assert!(
        skill_dir.join("OLD.md").exists(),
        "old version intact on deny"
    );

    let ok = tool
        .execute(
            r#"{"name": "replace-me", "registry": "stub", "force": true}"#,
            &ctx,
        )
        .await
        .unwrap();
    assert!(ok.contains("installed successfully"));
    assert!(
        !skill_dir.join("OLD.md").exists(),
        "old content removed by force"
    );
    let lock_text =
        std::fs::read_to_string(std::path::Path::new(&ws).join("skills.lock.json")).unwrap();
    assert!(
        lock_text.contains("replace-me"),
        "lockfile records the forced reinstall, got: {}",
        lock_text
    );
}
