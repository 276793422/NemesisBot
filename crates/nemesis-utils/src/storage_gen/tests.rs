//! 存储世代化迁移链骨架测试（四条铁律逐条钉子 + 中断恢复用例）。
//!
//! 设计文档：`存储世代化迁移链_设计文档` 。
//! 临时目录 = 系统临时目录 + nanos 唯一名，测试尾韧性清理。

use super::*;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

/// 唯一临时目录（nanos 防撞）+ 清场闭包（Drop 兜底删目录）。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("nb_l15_{tag}_{nanos}"));
        std::fs::create_dir_all(&p).expect("临时目录创建失败");
        TempDir(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = crate::fsutil::remove_dir_all_resilient(&self.0);
    }
}

/// 写世代文件（构造协议形态的源数据）。
fn write_gen(dir: &Path, stem: &str, n: u32, lines: &[serde_json::Value]) -> PathBuf {
    let p = dir.join(generation_file_name(stem, n));
    write_jsonl(&p, lines).expect("世代文件写入失败");
    p
}

/// 读文件字节（源保持原样的字节级断言用）。
fn bytes_of(p: &Path) -> Vec<u8> {
    std::fs::read(p).expect("读文件失败")
}

/// 演示迁移单元：把每行加一个 `v` 字段 = to 值（可观察的行变换）。
struct AddFieldStep(u32, u32);

impl MigrationStep for AddFieldStep {
    fn from(&self) -> u32 {
        self.0
    }
    fn to(&self) -> u32 {
        self.1
    }
    fn encode(&self, mut src: Vec<Value>) -> Result<Vec<Value>, String> {
        for row in &mut src {
            row["v"] = json!(self.1);
        }
        Ok(src)
    }
}

/// validate 恒败的包装步（校验失败零发布用例）。
struct FailValidateStep(AddFieldStep);

impl MigrationStep for FailValidateStep {
    fn from(&self) -> u32 {
        self.0.from()
    }
    fn to(&self) -> u32 {
        self.0.to()
    }
    fn encode(&self, src: Vec<Value>) -> Result<Vec<Value>, String> {
        self.0.encode(src)
    }
    fn validate(&self, _rows: &[Value]) -> Result<(), String> {
        Err("故意失败".to_string())
    }
}

/// encode 恒返空（空目标拒绝用例）。
struct EmptyEncodeStep(u32, u32);

impl MigrationStep for EmptyEncodeStep {
    fn from(&self) -> u32 {
        self.0
    }
    fn to(&self) -> u32 {
        self.1
    }
    fn encode(&self, _src: Vec<Value>) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }
}

fn two_step_chain() -> Vec<Box<dyn MigrationStep>> {
    vec![Box::new(AddFieldStep(1, 2)), Box::new(AddFieldStep(2, 3))]
}

// ---------------------------------------------------------------------------
// 铁律①：命名探测
// ---------------------------------------------------------------------------

#[test]
fn detect_bare_file_is_v1_baseline() {
    let t = TempDir::new("detect_v1");
    write_gen(t.path(), "session", 1, &[json!({"i": 1})]);

    let d = detect_generations(t.path(), "session").expect("探测失败");
    assert!(!d.empty);
    assert_eq!(d.generations.len(), 1);
    assert_eq!(d.latest().map(|(g, _)| g), Some(1));
    assert_eq!(
        d.latest()
            .map(|(_, p)| p.file_name().unwrap().to_string_lossy().to_string()),
        Some("session.jsonl".to_string()),
        "v1 = 无世代段裸名"
    );
}

#[test]
fn detect_versioned_mixed_and_ignores_non_protocol() {
    let t = TempDir::new("detect_mix");
    write_gen(t.path(), "session", 1, &[json!({"i": 1})]);
    write_gen(t.path(), "session", 2, &[json!({"i": 2})]);
    write_gen(t.path(), "session", 3, &[json!({"i": 3})]);
    // 非协议形态：显式 v1 世代文件 / 非数字段 / 别的 stem —— 全部忽略。
    std::fs::write(t.path().join("session.v1.jsonl"), "{}\n").unwrap();
    std::fs::write(t.path().join("session.vX.jsonl"), "{}\n").unwrap();
    std::fs::write(t.path().join("other.v2.jsonl"), "{}\n").unwrap();

    let d = detect_generations(t.path(), "session").expect("探测失败");
    let gens: Vec<u32> = d.generations.iter().map(|(g, _)| *g).collect();
    assert_eq!(gens, vec![1, 2, 3], "升序 + 非协议形态忽略");
    assert_eq!(d.latest().map(|(g, _)| g), Some(3));
}

#[test]
fn detect_empty_dir_and_missing_dir() {
    let t = TempDir::new("detect_empty");
    let d = detect_generations(t.path(), "session").expect("探测失败");
    assert!(d.empty);
    assert!(d.latest().is_none());

    let missing = t.path().join("no_such_dir");
    let d = detect_generations(&missing, "session").expect("不存在目录=空探测不 Err");
    assert!(d.empty);
}

// ---------------------------------------------------------------------------
// 铁律②③：逐级链 + 独占发布（源保持原样）
// ---------------------------------------------------------------------------

#[test]
fn chain_runs_stepwise_and_preserves_sources_verbatim() {
    let t = TempDir::new("chain_stepwise");
    let v1_rows = vec![json!({"i": 1}), json!({"i": 2})];
    let v1_path = write_gen(t.path(), "session", 1, &v1_rows);
    let v1_bytes = bytes_of(&v1_path);

    let out =
        migrate_to_latest(t.path(), "session", &two_step_chain(), 3, false).expect("链迁移失败");
    assert_eq!(out.from_generation, 1);
    assert_eq!(out.to_generation, 3);
    assert_eq!(out.steps.len(), 2, "两步链各 own 一步");
    assert_eq!(
        out.published_path.as_ref().unwrap().file_name().unwrap(),
        "session.v3.jsonl"
    );

    // 铁律③④：源世代保持原样——v1/v2 字节级不变。
    assert_eq!(bytes_of(&v1_path), v1_bytes, "v1 发布后必须原样保留");
    let v2_path = t.path().join("session.v2.jsonl");
    assert!(v2_path.exists(), "v2 已发布");
    let v2_rows = read_jsonl_strict(&v2_path).unwrap();
    assert!(
        v2_rows.iter().all(|r| r["v"] == json!(2)),
        "v2 行经第一步编码: {v2_rows:?}"
    );
    let v3_rows = read_jsonl_strict(&t.path().join("session.v3.jsonl")).unwrap();
    assert_eq!(v3_rows.len(), 2);
    assert!(
        v3_rows.iter().all(|r| r["v"] == json!(3)),
        "v3 行经第二步编码: {v3_rows:?}"
    );
    // 三文件并存（v1 原样 + v2 + v3）。
    let d = detect_generations(t.path(), "session").unwrap();
    assert_eq!(
        d.generations.iter().map(|(g, _)| *g).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

// ---------------------------------------------------------------------------
// 铁律④：未来版本拒绝 + 中断恢复
// ---------------------------------------------------------------------------

#[test]
fn future_generation_rejected_without_touching_disk() {
    let t = TempDir::new("future_gen");
    let v99_path = write_gen(t.path(), "session", 99, &[json!({"i": 1})]);
    let v99_bytes = bytes_of(&v99_path);

    let err = migrate_to_latest(t.path(), "session", &two_step_chain(), 3, false).unwrap_err();
    assert!(
        matches!(
            err,
            GenError::FutureGeneration {
                found: 99,
                target: 3
            }
        ),
        "实际 {err:?}"
    );
    assert_eq!(bytes_of(&v99_path), v99_bytes, "拒绝时零写盘");
}

#[test]
fn migrating_residue_self_heals_and_republishes() {
    let t = TempDir::new("residue_heal");
    write_gen(t.path(), "session", 1, &[json!({"i": 1})]);
    // 中断残留：v2 临时名已落盘（encode 后 rename 前进程终止的产物），
    // 内容是半截垃圾——残留永不采信，清扫重走。
    let residue = t.path().join("session.v2.jsonl.migrating");
    std::fs::write(&residue, "{\"half").unwrap();
    assert!(residue.exists());

    let out = migrate_to_latest(t.path(), "session", &two_step_chain(), 3, false)
        .expect("残留自愈迁移失败");
    assert_eq!(out.to_generation, 3);
    assert!(!residue.exists(), "临时名残留必须被清扫");
    // 重走后 v2 内容 = 正确编码产物（不是垃圾残留）。
    let v2_rows = read_jsonl_strict(&t.path().join("session.v2.jsonl")).unwrap();
    assert!(
        v2_rows.iter().all(|r| r["v"] == json!(2)),
        "v2 应为重走的正确产物: {v2_rows:?}"
    );
}

#[test]
fn published_generation_resumes_chain_untouched() {
    let t = TempDir::new("resume_chain");
    let v1_path = write_gen(t.path(), "session", 1, &[json!({"i": 1})]);
    // 已发布中断形态：v2 已发布（按协议正确编码），v3 未开始。
    let v2_path = write_gen(t.path(), "session", 2, &[json!({"i": 1, "v": 2})]);
    let v1_bytes = bytes_of(&v1_path);
    let v2_bytes = bytes_of(&v2_path);

    let out =
        migrate_to_latest(t.path(), "session", &two_step_chain(), 3, false).expect("续走迁移失败");
    assert_eq!(out.from_generation, 2, "从 detect 最高世代续走");
    assert_eq!(out.to_generation, 3);
    assert_eq!(out.steps.len(), 1, "只走缺的一步");
    assert_eq!(bytes_of(&v1_path), v1_bytes, "v1 永不触碰");
    assert_eq!(bytes_of(&v2_path), v2_bytes, "已发布 v2 永不触碰");
    assert!(t.path().join("session.v3.jsonl").exists());
}

// ---------------------------------------------------------------------------
// 失败路径：链缺步 / 空目标 / 校验失败零发布
// ---------------------------------------------------------------------------

#[test]
fn missing_step_reports_honestly() {
    let t = TempDir::new("missing_step");
    write_gen(t.path(), "session", 1, &[json!({"i": 1})]);
    // 链只有（缺 v1→v2）。
    let broken: Vec<Box<dyn MigrationStep>> = vec![Box::new(AddFieldStep(2, 3))];
    let err = migrate_to_latest(t.path(), "session", &broken, 3, false).unwrap_err();
    assert!(
        matches!(err, GenError::MissingStep { from: 1 }),
        "实际 {err:?}"
    );
}

#[test]
fn empty_encode_rejected_before_publish() {
    let t = TempDir::new("empty_target");
    write_gen(t.path(), "session", 1, &[json!({"i": 1})]);
    let chain: Vec<Box<dyn MigrationStep>> = vec![Box::new(EmptyEncodeStep(1, 2))];
    let err = migrate_to_latest(t.path(), "session", &chain, 2, false).unwrap_err();
    assert!(
        matches!(err, GenError::EmptyTarget { from: 1 }),
        "实际 {err:?}"
    );
    assert!(
        !t.path().join("session.v2.jsonl").exists(),
        "空目标不得发布"
    );
}

#[test]
fn validate_failure_cleans_tmp_and_publishes_nothing() {
    let t = TempDir::new("validate_fail");
    write_gen(t.path(), "session", 1, &[json!({"i": 1})]);
    let chain: Vec<Box<dyn MigrationStep>> = vec![Box::new(FailValidateStep(AddFieldStep(1, 2)))];
    let err = migrate_to_latest(t.path(), "session", &chain, 2, false).unwrap_err();
    assert!(
        matches!(err, GenError::ValidateFailed { from: 1, .. }),
        "实际 {err:?}"
    );
    assert!(
        !t.path().join("session.v2.jsonl").exists(),
        "校验败不得发布"
    );
    let no_residue = !std::fs::read_dir(t.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().ends_with(".migrating"));
    assert!(no_residue, "临时名必须清扫");
}

// ---------------------------------------------------------------------------
// read_only / 幂等 / 并发重入
// ---------------------------------------------------------------------------

#[test]
fn read_only_reports_plan_and_writes_nothing() {
    let t = TempDir::new("read_only");
    write_gen(t.path(), "session", 1, &[json!({"i": 1})]);
    let before: Vec<_> = std::fs::read_dir(t.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    let out =
        migrate_to_latest(t.path(), "session", &two_step_chain(), 3, true).expect("read_only 失败");
    assert!(out.read_only);
    assert_eq!(out.steps.len(), 2, "报告计划两步");
    assert_eq!(out.steps[0].from, 1);
    assert_eq!(out.steps[1].to, 3);
    assert!(out.published_path.is_none(), "read_only 无发布");

    let after: Vec<_> = std::fs::read_dir(t.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(before, after, "read_only 零写盘（连残留清扫都不做）");
}

#[test]
fn already_latest_is_noop() {
    let t = TempDir::new("noop");
    let v1_path = write_gen(t.path(), "session", 1, &[json!({"i": 1})]);
    let v1_bytes = bytes_of(&v1_path);

    let out =
        migrate_to_latest(t.path(), "session", &two_step_chain(), 1, false).expect("幂等调用失败");
    assert_eq!(out.from_generation, 1);
    assert_eq!(out.to_generation, 1);
    assert!(out.steps.is_empty());
    assert!(out.published_path.is_none());
    assert_eq!(bytes_of(&v1_path), v1_bytes, "零执行零触碰");
}

#[test]
fn reentry_with_published_target_is_idempotent() {
    let t = TempDir::new("reentry");
    write_gen(t.path(), "session", 1, &[json!({"i": 1})]);
    // 并发先到者已发布 v2（内容不同——证明后到者没有覆盖它）。
    let v2_path = write_gen(t.path(), "session", 2, &[json!({"i": 1, "v": 999})]);
    let v2_bytes = bytes_of(&v2_path);

    // 后到者从 detect 最高世代（v2）起步——重入路径天然幂等（链从 v2 续走）。
    let out =
        migrate_to_latest(t.path(), "session", &two_step_chain(), 3, false).expect("重入失败");
    assert_eq!(out.from_generation, 2);
    assert_eq!(bytes_of(&v2_path), v2_bytes, "已发布 v2 未被覆盖");
    assert!(t.path().join("session.v3.jsonl").exists());
}

#[test]
fn rename_conflict_on_existing_target_takes_idempotent_arm() {
    let t = TempDir::new("rename_conflict");
    write_gen(t.path(), "session", 1, &[json!({"i": 1})]);
    let v2_path = t.path().join("session.v2.jsonl");

    // 并发窗口模拟：detect 之后、rename 之前另一进程抢先发布了 v2——
    // 在 encode 里制造（预置文件会被 detect 看到，测不到冲突臂）。
    struct RaceStep(u32, u32, PathBuf);
    impl MigrationStep for RaceStep {
        fn from(&self) -> u32 {
            self.0
        }
        fn to(&self) -> u32 {
            self.1
        }
        fn encode(&self, mut src: Vec<Value>) -> Result<Vec<Value>, String> {
            for row in &mut src {
                row["v"] = json!(self.1);
            }
            write_jsonl(&self.2, &[json!({"i": 1, "v": 42})]).map_err(|e| e.to_string())?;
            Ok(src)
        }
    }
    let chain: Vec<Box<dyn MigrationStep>> = vec![Box::new(RaceStep(1, 2, v2_path.clone()))];
    let out = migrate_to_latest(t.path(), "session", &chain, 2, false).expect("幂等臂失败");
    assert_eq!(out.steps.len(), 1, "冲突步按幂等臂完成");
    let published = std::fs::read_to_string(&v2_path).unwrap();
    assert_eq!(
        published.trim(),
        "{\"i\":1,\"v\":42}",
        "幂等臂不得覆盖并发者已发布内容"
    );
    assert!(
        !t.path().join("session.v2.jsonl.migrating").exists(),
        "幂等臂清扫临时名"
    );
}

// ---------------------------------------------------------------------------
// 坏行容忍边界：源世代坏行跳过（铁律④迁移器不猜），临时名坏行严格拦
// ---------------------------------------------------------------------------

#[test]
fn source_corrupt_tail_line_is_tolerated_by_encode_input() {
    let t = TempDir::new("corrupt_tail");
    // v1 末行是未密封半行（消费方读路径容忍的形态——read_chat_log 先例）。
    let v1_path = t.path().join("session.jsonl");
    std::fs::write(&v1_path, "{\"i\":1}\n{\"i\":2}\n{\"half\n").unwrap();

    // 坏行被设施剥除 → encode 只见 2 行 → v2 发布产物恰 2 行（无坏行无空行）。
    let chain: Vec<Box<dyn MigrationStep>> = vec![Box::new(AddFieldStep(1, 2))];
    let out = migrate_to_latest(t.path(), "session", &chain, 2, false).expect("坏行容忍迁移失败");
    assert_eq!(out.to_generation, 2);
    let v2_rows = read_jsonl_strict(&t.path().join("session.v2.jsonl")).unwrap();
    assert_eq!(v2_rows.len(), 2, "坏行剥除后发布产物 2 行: {v2_rows:?}");
    assert!(
        v2_rows.iter().all(|r| r["v"] == json!(2)),
        "全部行经编码: {v2_rows:?}"
    );
}
