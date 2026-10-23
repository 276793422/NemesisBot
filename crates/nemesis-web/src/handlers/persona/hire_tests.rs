//! 集群聘用（shop.hire，2026-10-23）测试：strip/build 映射纯函数、备份 zip、
//! 写盘主体（无 ctx，temp workspace 文件级验证）。
//!
//! - `strip_system_prefix`：下载形态剥离 / 手改兜底全文 / 纯模板产出空
//! - `build_cluster_soul`：追加顺序与分隔 / 空 tools 跳过 / 失配零丢失兜底
//! - `read_persona_display_name`：PERSONA.json name 读取 + 三种回落
//! - `backup_cluster_persona_files`：全缺 None / 按存在备份 + 内容回验 /
//!   非 UTF-8 字节与 peers.toml 照备 / 独占锁 fail-closed（win）/
//!   同毫秒重入不覆盖
//! - `hire_write_cluster_persona`：端到端——IDENTITY 1:1、SOUL 拼接、
//!   EXPERTISE 不动、peers.toml 只换 name、备份产出、缺文件诚实报错
//!
//! 决策契约：docs/REPORT/2026-10-23_集群聘用需求评估与决策.md

use super::*;

// -----------------------------------------------------------------------
// strip_system_prefix
// -----------------------------------------------------------------------

#[test]
fn strip_removes_download_shape_prefix() {
    // 超市下载写入形态：format!("{}\n{}", DEFAULT, extra)
    let extra = "---\n\n# 某人格 专用工作流\n\n干核心活。\n";
    let file = format!("{}\n{}", DEFAULT_AGENT_MD, extra);
    assert_eq!(strip_system_prefix(&file, DEFAULT_AGENT_MD), extra);
}

#[test]
fn strip_falls_back_to_full_content_on_prefix_mismatch() {
    // 用户手改过（前缀不匹配）→ 整个文件原样返回（零丢失兜底）
    let hand_edited = "# 我自己写的 AGENT\n\n内容在这里。";
    assert_eq!(
        strip_system_prefix(hand_edited, DEFAULT_AGENT_MD),
        hand_edited
    );
}

#[test]
fn strip_pure_default_yields_empty() {
    // tools_extra 为空的下载形态：TOOLS.md == DEFAULT_TOOLS_MD 精确相等
    assert_eq!(strip_system_prefix(DEFAULT_TOOLS_MD, DEFAULT_TOOLS_MD), "");
}

// -----------------------------------------------------------------------
// build_cluster_soul
// -----------------------------------------------------------------------

#[test]
fn soul_appends_agent_and_tools_extras_in_order() {
    let soul = "# 灵魂\n\n原则一。";
    let agent = format!("{}\n\n---\n\n# X 专用工作流\n\n干活。", DEFAULT_AGENT_MD);
    let tools = format!(
        "{}\n\n---\n\n# X 工具偏好\n\n偏好 ripgrep。",
        DEFAULT_TOOLS_MD
    );
    let out = build_cluster_soul(soul, &agent, &tools);
    assert!(out.starts_with("# 灵魂\n\n原则一。"), "{out}");
    let parts: Vec<&str> = out.split("\n\n---\n\n").collect();
    assert_eq!(parts.len(), 3, "SOUL + agent + tools 三段: {out}");
    assert!(parts[1].contains("# X 专用工作流"));
    assert!(parts[2].contains("# X 工具偏好"));
}

#[test]
fn soul_skips_empty_tools_extra() {
    let soul = "# 灵魂";
    let agent = format!("{}\n\n---\n\n# X 专用工作流\n\n干活。", DEFAULT_AGENT_MD);
    let tools = DEFAULT_TOOLS_MD.to_string(); // 下载时 tools_extra 为空的形态
    let out = build_cluster_soul(soul, &agent, &tools);
    assert!(out.contains("# X 专用工作流"), "{out}");
    assert_eq!(
        out.matches("\n\n---\n\n").count(),
        1,
        "只有 agent 一段分隔: {out}"
    );
}

#[test]
fn soul_fallback_keeps_everything_when_prefix_mismatches() {
    // 零丢失兜底：手改过的 AGENT/TOOLS 全文进 SOUL（宁可多塞不丢内容）
    let soul = "# 灵魂";
    let agent = "# 手写的 agent 全文";
    let tools = "# 手写的 tools 全文";
    let out = build_cluster_soul(soul, agent, tools);
    assert!(out.contains(soul), "{out}");
    assert!(out.contains(agent), "{out}");
    assert!(out.contains(tools), "{out}");
}

// -----------------------------------------------------------------------
// read_persona_display_name
// -----------------------------------------------------------------------

#[test]
fn display_name_reads_persona_json_and_falls_back_to_id() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    // 缺失 → id
    assert_eq!(read_persona_display_name(p, "some-id"), "some-id");
    // name 为空白 → id
    std::fs::write(p.join("PERSONA.json"), r#"{"name": "  ", "id": "some-id"}"#).unwrap();
    assert_eq!(read_persona_display_name(p, "some-id"), "some-id");
    // name 正常
    std::fs::write(
        p.join("PERSONA.json"),
        r#"{"name": "后端架构师", "id": "some-id"}"#,
    )
    .unwrap();
    assert_eq!(read_persona_display_name(p, "some-id"), "后端架构师");
    // 非法 JSON → id
    std::fs::write(p.join("PERSONA.json"), "not json").unwrap();
    assert_eq!(read_persona_display_name(p, "some-id"), "some-id");
}

// -----------------------------------------------------------------------
// backup_cluster_persona_files
// -----------------------------------------------------------------------

#[test]
fn backup_returns_none_when_cluster_persona_files_all_missing() {
    let ws = tempfile::tempdir().unwrap();
    assert_eq!(
        backup_cluster_persona_files(ws.path().to_str().unwrap()).unwrap(),
        None
    );
}

#[test]
fn backup_zips_present_files_with_contents_intact() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let cluster = root.join("cluster");
    std::fs::create_dir_all(&cluster).unwrap();
    std::fs::write(cluster.join("IDENTITY.md"), "我是身份").unwrap();
    std::fs::write(cluster.join("SOUL.md"), "我是灵魂").unwrap();
    // EXPERTISE 缺席 → 只备两件

    let rel = backup_cluster_persona_files(root.to_str().unwrap())
        .unwrap()
        .expect("应有备份");
    assert!(rel.starts_with("cluster/backups/persona_"), "{rel}");
    assert!(rel.ends_with(".zip"), "{rel}");

    let zip_path = root.join(&rel);
    assert!(zip_path.exists(), "备份 zip 应存在: {}", zip_path.display());

    // 解开验证条目与内容（zip 9 name() 返回 Result）
    let f = std::fs::File::open(&zip_path).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(f)).unwrap();
    assert_eq!(archive.len(), 2, "只备存在的两件");
    let mut identity = archive.by_name("IDENTITY.md").unwrap();
    let mut buf = String::new();
    std::io::Read::read_to_string(&mut identity, &mut buf).unwrap();
    assert_eq!(buf, "我是身份");
    drop(identity);
    let mut soul = archive.by_name("SOUL.md").unwrap();
    let mut buf = String::new();
    std::io::Read::read_to_string(&mut soul, &mut buf).unwrap();
    assert_eq!(buf, "我是灵魂");
}

#[test]
fn backup_collision_gets_numeric_suffix_not_overwrite() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let cluster = root.join("cluster");
    std::fs::create_dir_all(&cluster).unwrap();
    std::fs::write(cluster.join("IDENTITY.md"), "v1").unwrap();
    let rel1 = backup_cluster_persona_files(root.to_str().unwrap())
        .unwrap()
        .unwrap();
    let rel2 = backup_cluster_persona_files(root.to_str().unwrap())
        .unwrap()
        .unwrap();
    assert_ne!(rel1, rel2, "同毫秒两次备份不得同名覆盖");
    assert!(root.join(&rel2).exists());
}

#[test]
fn backup_keeps_non_utf8_and_peers_toml_bytes_intact() {
    // 非 UTF-8 手改文件必须照备（字节级，静默跳过 = 覆盖即丢）；peers.toml
    // 是 hire 改 name 的第三文件，回滚需要它，一并入包。
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let cluster = root.join("cluster");
    std::fs::create_dir_all(&cluster).unwrap();
    let raw: &[u8] = &[0xFF_u8, 0xFE, b'#', b'x']; // 非法 UTF-8（BOM 前缀形态）
    std::fs::write(cluster.join("IDENTITY.md"), raw).unwrap();
    std::fs::write(
        cluster.join("peers.toml"),
        "[node]\nid = \"node-a\"\nname = \"旧名字\"\n",
    )
    .unwrap();

    let rel = backup_cluster_persona_files(root.to_str().unwrap())
        .unwrap()
        .expect("存在文件必须备份成功");
    let f = std::fs::File::open(root.join(&rel)).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(f)).unwrap();
    assert_eq!(archive.len(), 2, "IDENTITY + peers.toml 两件");
    let mut id = archive.by_name("IDENTITY.md").unwrap();
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut id, &mut buf).unwrap();
    assert_eq!(buf, raw, "非 UTF-8 字节必须原样保留");
    drop(id);
    let mut peers = archive.by_name("peers.toml").unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(&mut peers, &mut s).unwrap();
    assert!(s.contains("旧名字"), "{s}");
}

#[cfg(windows)]
#[test]
fn backup_fails_closed_when_existing_file_unreadable() {
    // fail-closed：存在但读不出（独占锁）→ Err abort，绝不在无备份情况下覆盖
    use std::os::windows::fs::OpenOptionsExt;
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let cluster = root.join("cluster");
    std::fs::create_dir_all(&cluster).unwrap();
    std::fs::write(cluster.join("IDENTITY.md"), "锁住的旧身份").unwrap();
    let _guard = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0) // 独占：后续 fs::read 撞 ERROR_SHARING_VIOLATION
        .open(cluster.join("IDENTITY.md"))
        .unwrap();
    let err = backup_cluster_persona_files(root.to_str().unwrap()).unwrap_err();
    assert!(err.contains("IDENTITY.md"), "{err}");
    assert!(err.contains("拒绝在无备份情况下覆盖"), "{err}");
    // 什么都没写出来
    assert!(!root.join("cluster").join("backups").exists());
}

// -----------------------------------------------------------------------
// hire_write_cluster_persona（端到端文件级）
// -----------------------------------------------------------------------

/// 造一个模拟超市下载产物的 personas/<id>/ 四件套。
fn seed_persona(
    root: &std::path::Path,
    id: &str,
    name: &str,
    persona_json: &str,
) -> std::path::PathBuf {
    let pdir = root.join("personas").join(id);
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(pdir.join("IDENTITY.md"), format!("# 身份\n{name}。")).unwrap();
    std::fs::write(pdir.join("SOUL.md"), "# 灵魂\n严谨。").unwrap();
    std::fs::write(
        pdir.join("AGENT.md"),
        format!("{DEFAULT_AGENT_MD}\n\n---\n\n# {name} 专用工作流\n\n先读后写。"),
    )
    .unwrap();
    std::fs::write(pdir.join("TOOLS.md"), DEFAULT_TOOLS_MD).unwrap(); // tools_extra 空
    std::fs::write(pdir.join("PERSONA.json"), persona_json).unwrap();
    pdir
}

#[test]
fn hire_writes_cluster_files_and_updates_node_name_only() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let root_str = root.to_str().unwrap();

    let pdir = seed_persona(
        root,
        "backend-arch",
        "后端架构师",
        r#"{"name": "后端架构师", "id": "backend-arch"}"#,
    );

    // 既有集群人格 + peers.toml 身份（含不可碰的字段）
    let cluster = root.join("cluster");
    std::fs::create_dir_all(&cluster).unwrap();
    std::fs::write(cluster.join("IDENTITY.md"), "旧身份").unwrap();
    std::fs::write(cluster.join("SOUL.md"), "旧灵魂").unwrap();
    std::fs::write(cluster.join("EXPERTISE.md"), "旧专业方法论").unwrap();
    let peers = cluster.join("peers.toml");
    std::fs::write(
        &peers,
        r#"[node]
id = "node-a"
name = "旧名字"
role = "worker"
category = "development"
tags = ["rust"]
"#,
    )
    .unwrap();

    let (name, backup) = hire_write_cluster_persona(root_str, &pdir, "backend-arch").unwrap();
    assert_eq!(name, "后端架构师");
    assert!(backup.is_some(), "覆盖了旧文件，必须有备份");
    // 备份完整性：hire 触碰的全部既有文件（三件 + peers.toml）都进包
    {
        let f = std::fs::File::open(root.join(backup.as_ref().unwrap())).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::BufReader::new(f)).unwrap();
        assert_eq!(
            archive.len(),
            4,
            "IDENTITY/SOUL/EXPERTISE/peers.toml 四件: 备份必须覆盖 hire 触碰的全部文件"
        );
        assert!(archive.by_name("peers.toml").is_ok());
    }

    // IDENTITY 1:1
    assert_eq!(
        std::fs::read_to_string(cluster.join("IDENTITY.md")).unwrap(),
        "# 身份\n后端架构师。"
    );
    // SOUL = 新拼接（原 SOUL + agent extra；tools 空不追加）
    let new_soul = std::fs::read_to_string(cluster.join("SOUL.md")).unwrap();
    assert!(new_soul.starts_with("# 灵魂\n严谨。"), "{new_soul}");
    assert!(new_soul.contains("# 后端架构师 专用工作流"), "{new_soul}");
    // EXPERTISE 保守不动
    assert_eq!(
        std::fs::read_to_string(cluster.join("EXPERTISE.md")).unwrap(),
        "旧专业方法论"
    );
    // peers.toml：name 换，其余原值
    let cfg = nemesis_cluster::cluster_config::load_static_config(&peers).unwrap();
    assert_eq!(cfg.node.name, "后端架构师");
    assert_eq!(cfg.node.id, "node-a");
    assert_eq!(cfg.node.role, "worker");
    assert_eq!(cfg.node.category, "development");
    assert_eq!(cfg.node.tags, vec!["rust".to_string()]);
}

#[test]
fn hire_creates_peers_toml_when_missing_and_skips_backup_when_cluster_empty() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let root_str = root.to_str().unwrap();
    let pdir = seed_persona(root, "p1", "P1", r#"{"name": "P1"}"#);

    // cluster/ 目录整体不存在
    let (name, backup) = hire_write_cluster_persona(root_str, &pdir, "p1").unwrap();
    assert_eq!(name, "P1");
    assert_eq!(backup, None, "三件全缺 → 无备份");

    let peers = root.join("cluster").join("peers.toml");
    let cfg = nemesis_cluster::cluster_config::load_static_config(&peers).unwrap();
    assert_eq!(cfg.node.name, "P1");
    // IDENTITY/SOUL 照常落盘
    assert!(root.join("cluster").join("IDENTITY.md").exists());
    assert!(root.join("cluster").join("SOUL.md").exists());
}

#[test]
fn hire_errors_loudly_when_persona_file_missing() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    let pdir = root.join("personas").join("broken");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(pdir.join("IDENTITY.md"), "i").unwrap();
    // SOUL/AGENT/TOOLS 缺失
    let err = hire_write_cluster_persona(root.to_str().unwrap(), &pdir, "broken").unwrap_err();
    assert!(
        err.contains("personas/broken"),
        "错误应指明缺失文件来源: {err}"
    );
    // 未写任何 cluster 文件（先读全四件再动手）
    assert!(!root.join("cluster").join("IDENTITY.md").exists());
    assert!(!root.join("cluster").join("SOUL.md").exists());
}
