use super::*;

#[test]
fn sanitize_rejects_short_and_strips_controls() {
    assert!(sanitize_input("太短").is_err());
    let r = sanitize_input(
            "这是一段足够长的有效岗位描述文本，用于通过最小长度校验门槛，这里再多写一些内容确保超过四十个字符 abcdefgh",
        )
        .unwrap();
    assert!(!r.contains('\u{7f}'));
    assert!(!r.contains('\r'));
}

#[test]
fn validate_enforces_role_enum() {
    let mut pkg = PersonaPackage {
        node_name: "x".into(),
        display_name: "X".into(),
        emoji: "🤖".into(),
        role: "admin".into(),
        category: "dev".into(),
        tags: vec![" a ".into()],
        identity_md: "# X\nwho".into(),
        soul_md: "# Rules\n- a".into(),
        expertise_md: String::new(),
        coverage: None,
    };
    assert!(validate(&mut pkg).is_err());
    pkg.role = "worker".into();
    assert!(validate(&mut pkg).is_ok());
    assert_eq!(pkg.tags, vec!["a".to_string()]);
}

#[test]
fn unwrap_single_key_handles_wrapped_args() {
    let wrapped =
        serde_json::json!({ "emit_cluster_persona": { "identity_md": "# x", "soul_md": "# y" } });
    let v = unwrap_single_key(wrapped);
    assert!(v.get("identity_md").is_some());
}

#[test]
fn extract_json_span_and_fence() {
    assert_eq!(extract_json_span("noise {\"a\":1} tail"), Some("{\"a\":1}"));
    let stripped = strip_code_fence("```json\n{\"a\":1}\n```");
    assert_eq!(stripped, "{\"a\":1}");
}

// ---- 程序确定性校验（机制硬骨架）单测 ----

fn sample_unit(id: &str, disposition: &str, entities: &[&str]) -> InformationUnit {
    InformationUnit {
        id: id.into(),
        content: format!("unit {id}"),
        unit_type: "tech_decision".into(),
        relevance: "high".into(),
        disposition: disposition.into(),
        drop_reason: None,
        key_entities: entities.iter().map(|s| s.to_string()).collect(),
    }
}

fn sample_pkg(identity: &str, soul: &str, expertise: &str) -> PersonaPackage {
    PersonaPackage {
        node_name: "x".into(),
        display_name: "X".into(),
        emoji: "🤖".into(),
        role: "worker".into(),
        category: "dev".into(),
        tags: vec![],
        identity_md: identity.into(),
        soul_md: soul.into(),
        expertise_md: expertise.into(),
        coverage: None,
    }
}

#[test]
fn entity_coverage_covered_when_all_entities_present() {
    let units = vec![sample_unit("u1", "expertise", &["RocketMQ", "事务消息"])];
    let pkg = sample_pkg("", "", "用 RocketMQ 事务消息做分布式事务");
    let entries = check_entity_coverage(&units, &pkg);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, CoverageStatus::Covered);
}

#[test]
fn entity_coverage_missing_when_entity_absent() {
    let units = vec![sample_unit("u1", "soul", &["分库分表"])];
    let pkg = sample_pkg("", "没有提到相关内容", "");
    let entries = check_entity_coverage(&units, &pkg);
    assert_eq!(entries[0].status, CoverageStatus::Missing);
    assert!(entries[0].reason.as_ref().unwrap().contains("分库分表"));
}

#[test]
fn entity_coverage_case_insensitive() {
    let units = vec![sample_unit("u1", "identity", &["kafka"])];
    let pkg = sample_pkg("我用 Kafka 做过消息系统", "", "");
    let entries = check_entity_coverage(&units, &pkg);
    assert_eq!(entries[0].status, CoverageStatus::Covered);
}

#[test]
fn segment_gap_reported_when_unit_count_zero() {
    let units = InformationUnits {
        units: vec![],
        segments: vec![
            InputSegment {
                id: "s1".into(),
                label: "技能".into(),
                unit_count: 3,
            },
            InputSegment {
                id: "s2".into(),
                label: "教育".into(),
                unit_count: 0,
            },
        ],
    };
    let gaps = check_segment_coverage(&units);
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].contains("教育"));
}

#[test]
fn report_program_missing_wins_over_audit() {
    // 程序判 Missing（实体没出现）→ 报告 Missing，即使审计说 covered 也不信。
    let units = vec![sample_unit("u1", "expertise", &["Redis"])];
    let prog = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Missing,
        location: Some("expertise".into()),
        reason: Some("缺 Redis".into()),
    }];
    let audit = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: Some("expertise".into()),
        reason: None,
    }];
    let report = build_coverage_report(&units, prog, audit, vec![]);
    assert_eq!(report.missing, 1);
    assert!(!report.is_complete());
}

#[test]
fn report_covered_but_audit_suspect_becomes_suspect_non_blocking() {
    // 程序 Covered（词在）+ 审计 Suspect（意思存疑）→ Suspect；suspect 不阻断完整性。
    let units = vec![sample_unit("u1", "expertise", &["Redis"])];
    let prog = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: Some("expertise".into()),
        reason: None,
    }];
    let audit = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Suspect,
        location: None,
        reason: Some("词在但意思没到位".into()),
    }];
    let report = build_coverage_report(&units, prog, audit, vec![]);
    assert_eq!(report.covered, 0);
    assert_eq!(report.suspect, 1);
    assert!(report.is_complete());
}

#[test]
fn report_complete_when_no_missing_and_no_gaps() {
    let units = vec![sample_unit("u1", "expertise", &["Redis"])];
    let prog = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: Some("expertise".into()),
        reason: None,
    }];
    let report = build_coverage_report(&units, prog, vec![], vec![]);
    assert_eq!(report.coverage_rate, 1.0);
    assert!(report.is_complete());
}

#[test]
fn report_skipped_counts_archive_and_drop() {
    let units = vec![
        sample_unit("u1", "expertise", &["Redis"]),
        InformationUnit {
            id: "u2".into(),
            content: "无关信息".into(),
            unit_type: "skill".into(),
            relevance: "none".into(),
            disposition: "drop".into(),
            drop_reason: Some("与人格无关".into()),
            key_entities: vec![],
        },
    ];
    let prog = vec![CoverageEntry {
        unit_id: "u1".into(),
        status: CoverageStatus::Covered,
        location: Some("expertise".into()),
        reason: None,
    }];
    let report = build_coverage_report(&units, prog, vec![], vec![]);
    assert_eq!(report.total, 2);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.covered, 1);
}
