//! knowledge 模块测试：重定向纯函数（命中/本地优先/非挂载/开关关/重叠）
//! + 声明注入（开关关=None/命中=声明/多层合并/full 模式/镜像 fallback）
//! + KnowledgeHandle mtime 缓存与 fail-closed。

use super::*;
use nemesis_config::knowledge::{KnowledgePair, TIANGONG_LOAD_FULL, TIANGONG_LOAD_POINTER};
use std::path::PathBuf;

fn tmp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("nemesis_knowledge_{}_{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn pair(code_root: &str, info_root: &str, mode: &str) -> KnowledgePair {
    KnowledgePair {
        code_root: code_root.to_string(),
        info_root: info_root.to_string(),
        tiangong_load: mode.to_string(),
    }
}

// ---------- redirect_instruction_path ----------

#[test]
fn test_redirect_disabled_is_identity() {
    let cfg = KnowledgeConfig {
        enabled: false,
        pairs: vec![pair("C:/code", "C:/info", TIANGONG_LOAD_POINTER)],
    };
    let p = Path::new("C:/code/src/AGENTS.md");
    assert_eq!(redirect_instruction_path(p, &cfg), p.to_path_buf());
}

#[test]
fn test_redirect_hits_missing_local_three_files() {
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair("C:/code", "C:/info", TIANGONG_LOAD_POINTER)],
    };
    // 三件套全部命中改写（本地不存在）。
    for name in INSTRUCTION_FILE_NAMES {
        let p = PathBuf::from(format!("C:/code/src/{}", name));
        assert_eq!(
            redirect_instruction_path(&p, &cfg),
            PathBuf::from(format!("C:/info/src/{}", name)),
            "三件套 {name} 应命中镜像"
        );
    }
    // 非三件套文件名不动。
    let other = Path::new("C:/code/src/main.rs");
    assert_eq!(redirect_instruction_path(other, &cfg), other.to_path_buf());
    // code_root 之外的三件套不动。
    let outside = Path::new("C:/elsewhere/AGENTS.md");
    assert_eq!(
        redirect_instruction_path(outside, &cfg),
        outside.to_path_buf()
    );
}

#[test]
fn test_redirect_local_exists_wins_overlay() {
    let dir = tmp_dir("overlay_local");
    // 本地文件在场。
    let code = dir.join("code");
    std::fs::create_dir_all(code.join("src")).unwrap();
    let local = code.join("src").join("AGENTS.md");
    std::fs::write(&local, "local").unwrap();

    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            "Z:/info_does_not_matter",
            TIANGONG_LOAD_POINTER,
        )],
    };
    assert_eq!(redirect_instruction_path(&local, &cfg), local);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_redirect_code_root_exact_layer() {
    // 父目录 == code_root（根层三件套）同样命中。
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair("C:/code", "C:/info", TIANGONG_LOAD_POINTER)],
    };
    let p = Path::new("C:/code/TIANGONG.md");
    assert_eq!(
        redirect_instruction_path(p, &cfg),
        PathBuf::from("C:/info/TIANGONG.md")
    );
}

#[test]
fn test_redirect_relative_path_identity() {
    // 相对路径（无界形态可能残留）父目录不匹配绝对 code_root → 原样。
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair("C:/code", "C:/info", TIANGONG_LOAD_POINTER)],
    };
    let p = Path::new("src/AGENTS.md");
    assert_eq!(redirect_instruction_path(p, &cfg), p.to_path_buf());
}

#[test]
fn test_redirect_half_component_no_false_prefix() {
    // C:/a/bc 不是 C:/a/b 的子树（组件级前缀，防半截字符串误判）。
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair("C:/a/b", "C:/info", TIANGONG_LOAD_POINTER)],
    };
    let p = Path::new("C:/a/bc/AGENTS.md");
    assert_eq!(redirect_instruction_path(p, &cfg), p.to_path_buf());
}

#[test]
fn test_redirect_windows_case_insensitive_subtree() {
    // Windows 子树匹配大小写不敏感（仅 cfg!(windows) 生效；其余平台
    // 敏感——断言按平台分叉）。
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair("C:/Code", "C:/info", TIANGONG_LOAD_POINTER)],
    };
    let p = Path::new("C:/code/src/CLAUDE.md");
    if cfg!(windows) {
        assert_eq!(
            redirect_instruction_path(p, &cfg),
            PathBuf::from("C:/info/src/CLAUDE.md")
        );
    } else {
        assert_eq!(redirect_instruction_path(p, &cfg), p.to_path_buf());
    }
}

#[test]
fn test_redirect_dotdot_components_lexically_normalized() {
    // 第五轮横扫 2026-11-27：LLM 入参带 `..`/`.` 组件（如
    // `proj/src/../TIANGONG.md`）→ 镜像路径词法折叠（不含 `..` 落地）。
    // 未归一时 checkpoint 的 safe_path 拒含 `..` 路径 → 快照读 None →
    // rewind 把声明当「不存在」走删除臂，会误删已有镜像文件；归一后
    // 声明路径即真实路径，快照/恢复/指纹全链对齐。
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair("C:/code", "C:/info", TIANGONG_LOAD_POINTER)],
    };
    let p = Path::new("C:/code/src/../TIANGONG.md");
    assert_eq!(
        redirect_instruction_path(p, &cfg),
        PathBuf::from("C:/info/TIANGONG.md"),
        "`..` 组件应折叠"
    );
    let p2 = Path::new("C:/code/./src/AGENTS.md");
    assert_eq!(
        redirect_instruction_path(p2, &cfg),
        PathBuf::from("C:/info/src/AGENTS.md"),
        "`.` 组件应剔除"
    );
    // BUG-20261127-047：净 `..` 越出 code 子树的形态（真实落点 `C:/x`，
    // 不在挂载子树内）——修复后 parent 先归一再匹配，归一落点（Windows
    // `C:/..` / Linux `..`）均不含 code 组件 → 不命中 → 透传。原断言
    // 产出越出 info_root 的镜像（Windows `C:/../x/TIANGONG.md`），恰是
    // 本 BUG 的逃逸形态；overlay 语义下子树外路径本就不该重定向，两平台
    // 统一透传，不再有平台分叉断言。
    let p3 = Path::new("C:/code/../../x/TIANGONG.md");
    assert_eq!(redirect_instruction_path(p3, &cfg), p3.to_path_buf());
}

#[test]
fn test_redirect_dotdot_net_escape_out_of_subtree_passthrough() {
    // BUG-20261127-047 红证：裸组件前缀匹配不折 `..`——净 `..` 超出后缀
    // 深度的路径**真实落点在 code 子树外**，仍误命中 → 镜像 rel 含越根
    // `..`，归一后落点越出 info_root。链路语义：围栏前臂（绝对入参）
    // 的越界产物靠围栏 backstop 兜底；围栏后臂（相对入参围栏放行后二次
    // 重定向）输出无人复检 → 写/读落在 info_root 之外。修复 = parent 先
    // 词法归一再做子树判定：归一后落点出子树 → 不命中 → 透传真实路径
    // （overlay 语义：子树外本来就不该重定向）。
    let dir = tmp_dir("redirect_escape");
    let code = dir.join("code");
    let info = dir.join("info");
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            &info.to_string_lossy(),
            TIANGONG_LOAD_POINTER,
        )],
    };
    // 形态一：code_root 根下两级净 `..` ×3——真实落点 dir/AGENTS.md
    // （code 子树外）。修复前镜像归一后落 dir/AGENTS.md（越出 info_root）。
    let p = code.join("a/b/../../../AGENTS.md");
    assert_eq!(redirect_instruction_path(&p, &cfg), p, "越子树形态应透传");

    // 形态二（设计常态）：code_root 挂更深的「工作区」子树内——真实落点
    // dir/ws/AGENTS.md 在边界内（若挂围栏照常放行）、在 code 子树外。
    // 修复前镜像落 dir/AGENTS.md（越出 info_root = dir/kb）。
    let cfg2 = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &dir.join("ws/proj").to_string_lossy(),
            &dir.join("kb").to_string_lossy(),
            TIANGONG_LOAD_POINTER,
        )],
    };
    let p2 = dir.join("ws/proj/a/b/../../../AGENTS.md");
    assert_eq!(
        redirect_instruction_path(&p2, &cfg2),
        p2,
        "越子树形态应透传"
    );

    // 非回归（合法形态行为不变）：`..` 折叠后仍落子树内 → 照常重定向，
    // 镜像不含 `..` 且恒在 info_root 内。
    let p3 = code.join("src/a/../AGENTS.md"); // 真实落点 code/src/AGENTS.md（子树内）
    assert_eq!(
        redirect_instruction_path(&p3, &cfg),
        info.join("src/AGENTS.md")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_maybe_redirect_none_handle_identity() {
    let p = Path::new("C:/code/AGENTS.md");
    assert_eq!(maybe_redirect_path(p, None), p.to_path_buf());
}

// ---------- knowledge_decl_section ----------
// 层锚定 = 各 pair code_root 侧项目层级（2026-11-26 UAT 修正——初版锚
// workspace root 在主对话形态永不命中）。fixture 一律构造 code_root
// 目录（本地/镜像命中都在它上面裁决）。

#[test]
fn test_decl_disabled_is_none() {
    let code = tmp_dir("decl_disabled");
    std::fs::write(code.join("TIANGONG.md"), "x").unwrap();
    let cfg = KnowledgeConfig::default();
    assert!(knowledge_decl_section(&cfg).is_none());
    let _ = std::fs::remove_dir_all(&code);
}

#[test]
fn test_decl_pointer_root_hit() {
    // 本地（code_root 根层）TIANGONG 在场 → pointer 声明（overlay 本地臂）。
    let code = tmp_dir("decl_root");
    std::fs::write(code.join("TIANGONG.md"), "x").unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            "C:/unused_info",
            TIANGONG_LOAD_POINTER,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.starts_with("# 天工"), "{section}");
    assert!(section.contains("本项目存在天工引导文件 TIANGONG.md"));
    assert!(section.contains("若有需求可读"));
    // 单层命中无括注。
    assert!(!section.contains("存在层级"), "{section}");
    let _ = std::fs::remove_dir_all(&code);
}

#[test]
fn test_decl_no_pairs_is_none() {
    // 无挂载 pair = 无层集合 = 段缺席（fail-closed 同族）。
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![],
    };
    assert!(knowledge_decl_section(&cfg).is_none());
}

#[test]
fn test_decl_no_hit_is_none() {
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair("C:/code", "C:/info", TIANGONG_LOAD_POINTER)],
    };
    assert!(knowledge_decl_section(&cfg).is_none());
}

#[test]
fn test_decl_multi_layer_merged_with_annotation() {
    // 本地根层 + src 子层双命中 → 一条声明 + 括注层级。
    let code = tmp_dir("decl_multi");
    std::fs::write(code.join("TIANGONG.md"), "root").unwrap();
    std::fs::create_dir_all(code.join("src")).unwrap();
    std::fs::write(code.join("src").join("TIANGONG.md"), "sub").unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            "C:/unused_info",
            TIANGONG_LOAD_POINTER,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert_eq!(section.matches("# 天工").count(), 1, "{section}");
    assert!(section.contains("存在层级：根目录、src"), "{section}");
    let _ = std::fs::remove_dir_all(&code);
}

#[test]
fn test_decl_mirror_fallback_hit() {
    // 主对话真实形态：code_root 与「workspace」完全分离（本 fixture 里
    // workspace 根本不存在——函数已不接 ws_root）。本地无 TIANGONG；
    // 镜像 src 子层有 → 单子层命中（第五轮横扫 2026-11-27：括注如实
    // 列举命中层——旧实现单子层命中无任何位置线索，LLM 只能盲试）。
    let dir = tmp_dir("decl_mirror");
    let info = dir.join("info");
    std::fs::create_dir_all(info.join("src")).unwrap();
    std::fs::write(info.join("src").join("TIANGONG.md"), "mirror").unwrap();
    let code = dir.join("code");
    std::fs::create_dir_all(code.join("src")).unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            &info.to_string_lossy(),
            TIANGONG_LOAD_POINTER,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.starts_with("# 天工"), "{section}");
    assert!(section.contains("存在层级：src"), "{section}");
    assert!(
        !section.contains("根目录"),
        "根层未命中不得谎报根目录: {section}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_decl_pointer_annotation_root_miss_subdirs_only() {
    // 根层未命中 + 两个子层命中 → 括注只列子层（旧实现无条件以「根目录」
    // 开头——括注谎报根目录，LLM 按声明去根层读 File not found）。
    let dir = tmp_dir("decl_root_miss");
    let info = dir.join("info");
    for sub in ["docs", "src"] {
        std::fs::create_dir_all(info.join(sub)).unwrap();
        std::fs::write(info.join(sub).join("TIANGONG.md"), "mirror").unwrap();
    }
    let code = dir.join("code");
    for sub in ["docs", "src"] {
        std::fs::create_dir_all(code.join(sub)).unwrap();
    }
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            &info.to_string_lossy(),
            TIANGONG_LOAD_POINTER,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.contains("存在层级：docs、src"), "{section}");
    assert!(
        !section.contains("根目录"),
        "根层未命中不得出现在括注: {section}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_decl_root_layer_key_subdir_collision_not_root_only() {
    // 第七轮横扫 2026-11-27（第十一处）：根层合并键原为中文字面「根目录」
    // ——用户 code_root 下恰有同名子目录且只有它命中时两层合并 →
    // root_only 假命中零括注（LLM 以为根层有 TIANGONG 直奔根层 File not
    // found——第五轮括注如实化修掉的行为从合并键绕回来）。修复 = 根层
    // 哨兵键（空串，子目录名永不为空）。期望：括注照实出现（子目录层
    // 独立命中，不再被吞进根层假命中）。
    let dir = tmp_dir("decl_root_key_collision");
    let code = dir.join("code");
    let info = dir.join("info");
    std::fs::create_dir_all(code.join("根目录")).unwrap();
    std::fs::create_dir_all(info.join("根目录")).unwrap();
    std::fs::write(info.join("根目录").join("TIANGONG.md"), "mirror").unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            &info.to_string_lossy(),
            TIANGONG_LOAD_POINTER,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.starts_with("# 天工"), "{section}");
    assert!(
        section.contains("存在层级：根目录"),
        "子目录「根目录」命中必须给括注（不得被根层合并键吞成假 root_only）: {section}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_decl_root_layer_key_dedup_display_names() {
    // 哨兵修复的显示名去重臂：根层与同名子目录「根目录」都命中 →
    // 两层显示名相同 → 去重后括注只出现一次「根目录」（重复字样无
    // 信息量；LLM 根层未读到按名探索子目录）。
    let dir = tmp_dir("decl_root_key_dedup");
    let code = dir.join("code");
    let info = dir.join("info");
    std::fs::create_dir_all(code.join("根目录")).unwrap();
    std::fs::write(code.join("TIANGONG.md"), "local root").unwrap();
    std::fs::create_dir_all(info.join("根目录")).unwrap();
    std::fs::write(info.join("根目录").join("TIANGONG.md"), "mirror").unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            &info.to_string_lossy(),
            TIANGONG_LOAD_POINTER,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.contains("存在层级：根目录"), "{section}");
    assert_eq!(
        section.matches("存在层级：根目录").count(),
        1,
        "同名层显示去重: {section}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_decl_pointer_root_only_hit_no_annotation() {
    // 根层唯一命中 = 最常见形态 → 声明零附加字节（既有口径保持）。
    let code = tmp_dir("decl_root_only");
    std::fs::write(code.join("TIANGONG.md"), "root").unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            "C:/unused_info",
            TIANGONG_LOAD_POINTER,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.starts_with("# 天工"), "{section}");
    assert!(!section.contains("存在层级"), "{section}");
    let _ = std::fs::remove_dir_all(&code);
}

#[test]
fn test_decl_mirror_root_and_sub_hit() {
    // 分离形态根层 + 子层镜像双命中 → 两层合并括注（初版此形态只在
    // ws_root==code_root 下可达——恰是 v1 不支持的项目会话形态；层锚
    // 改 code_root 后成为主对话可达的真实形态）。
    let dir = tmp_dir("decl_mirror_root_eq");
    let info = dir.join("info");
    std::fs::create_dir_all(info.join("src")).unwrap();
    std::fs::write(info.join("TIANGONG.md"), "root mirror").unwrap();
    std::fs::write(info.join("src").join("TIANGONG.md"), "sub mirror").unwrap();
    let code = dir.join("code");
    std::fs::create_dir_all(code.join("src")).unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            &info.to_string_lossy(),
            TIANGONG_LOAD_POINTER,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.contains("存在层级：根目录、src"), "{section}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_decl_full_mode_injects_content_with_path() {
    // full 本地臂：code_root 根层本地 TIANGONG + pair full → 全文注入。
    let code = tmp_dir("decl_full");
    std::fs::write(code.join("TIANGONG.md"), "# 地图\n层级一。").unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            "C:/unused_info",
            TIANGONG_LOAD_FULL,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.contains("全文注入，full 模式"), "{section}");
    assert!(section.contains("生效文件："), "{section}");
    assert!(section.contains("# 地图"), "{section}");
    let _ = std::fs::remove_dir_all(&code);
}

#[test]
fn test_decl_full_mode_oversize_degrades_to_pointer() {
    // 第九轮横扫 2026-11-27（第十三处）：full 模式体积护栏——超过
    // `TIANGONG_FULL_MAX_BYTES` 的根层 TIANGONG 不得全文注入（每轮全文
    // 进上下文 → provider 请求超限 → 会话不可用），诚实降级 pointer 声明
    // （LLM 仍按声明自己去读，功能降级不炸会话）。
    let code = tmp_dir("decl_full_oversize");
    let big = "# 超大地图\n".to_string() + &"x".repeat(TIANGONG_FULL_MAX_BYTES);
    std::fs::write(code.join("TIANGONG.md"), &big).unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            "C:/unused_info",
            TIANGONG_LOAD_FULL,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(
        !section.contains("全文注入"),
        "超限文件不得全文注入: {} bytes",
        big.len()
    );
    assert!(
        section.starts_with("# 天工"),
        "降级 pointer 声明: {section}"
    );
    assert!(
        !section.contains("超大地图"),
        "全文内容不得泄漏进声明: {section}"
    );

    // 边界内（恰好等于上限）照常全文注入。
    let edge =
        "# 边界地图\n".to_string() + &"x".repeat(TIANGONG_FULL_MAX_BYTES - "# 边界地图\n".len());
    std::fs::write(code.join("TIANGONG.md"), &edge).unwrap();
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(
        section.contains("全文注入，full 模式"),
        "恰好等于上限照常全文注入: {} bytes",
        edge.len()
    );
    let _ = std::fs::remove_dir_all(&code);
}

#[test]
fn test_full_section_read_backstop_oversize_degrades() {
    // 第十轮横扫 2026-11-27（第十四处 C 臂）：metadata 预检与 read 之间的
    // 替换竞窗（维护者恰好在此间保存超大文件——预检放行但内容已超限）无法
    // 确定性复现，回检谓词抽纯函数直测：超限内容降级 pointer、恰等于上限
    // 照常全文注入（与 metadata 预检臂同一谓词同一边界）。
    let path = PathBuf::from("TIANGONG.md");
    let oversize = "# 超大地图\n".to_string() + &"x".repeat(TIANGONG_FULL_MAX_BYTES);
    assert_eq!(
        render_full_section_guarded(&path, &oversize, "P"),
        Some("P".to_string()),
        "读后回检：超限内容降级 pointer 声明"
    );
    let head = "# 地图\n";
    let edge = format!("{head}{}", "x".repeat(TIANGONG_FULL_MAX_BYTES - head.len()));
    let out = render_full_section_guarded(&path, &edge, "P").unwrap();
    assert!(
        out.contains("全文注入，full 模式"),
        "读后回检边界：恰等于上限照常全文注入"
    );
    assert!(out.contains(&edge), "边界内容完整保留: {}", out.len());
}

#[test]
fn test_decl_full_mode_scoped_to_source_pair_not_any() {
    // 第六轮横扫 2026-11-27（第九处）：full 判定锚定**生效来源 pair**
    // ——pair0 pointer（本地根层 TIANGONG 在场）+ pair1 full（无根层
    // 生效文件）→ 循环第一命中 pair0、本地来源归属 pair0 → pointer
    // 声明。原实现本地臂 `any(pair full)` 会让 pair0 的本地文件被
    // pair1 的 full 配置误全文注入（pointer 意图违背 + 无谓 token）。
    let dir = tmp_dir("decl_full_scope");
    let code_a = dir.join("code_a");
    let code_b = dir.join("code_b");
    let info_b = dir.join("info_b");
    std::fs::create_dir_all(&code_a).unwrap();
    std::fs::create_dir_all(&code_b).unwrap();
    std::fs::create_dir_all(&info_b).unwrap();
    // pair0 本地根层文件；pair1 镜像侧只放子层（根层无生效文件）。
    std::fs::write(code_a.join("TIANGONG.md"), "# A 本地").unwrap();
    std::fs::create_dir_all(info_b.join("src")).unwrap();
    std::fs::write(info_b.join("src").join("TIANGONG.md"), "b sub").unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![
            pair(
                &code_a.to_string_lossy(),
                "Z:/unused_a",
                TIANGONG_LOAD_POINTER,
            ),
            pair(
                &code_b.to_string_lossy(),
                &info_b.to_string_lossy(),
                TIANGONG_LOAD_FULL,
            ),
        ],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.starts_with("# 天工"), "{section}");
    assert!(
        !section.contains("全文注入"),
        "pair0 是 pointer，不得被 pair1 的 full 配置误全文注入: {section}"
    );
    assert!(!section.contains("# A 本地"), "{section}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_decl_full_requires_pair_mode_pointer_stays_pointer() {
    // 本地 TIANGONG 在场但 pair 是 pointer → 只注入存在声明。
    let code = tmp_dir("decl_full_off");
    std::fs::write(code.join("TIANGONG.md"), "# 地图").unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            "C:/ui",
            TIANGONG_LOAD_POINTER,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.starts_with("# 天工"));
    assert!(!section.contains("全文注入"), "{section}");
    let _ = std::fs::remove_dir_all(&code);
}

#[test]
fn test_decl_full_mirror_source_uses_pair_mode() {
    // full 镜像臂：全文来自 info_root，生效路径 = 镜像路径。
    let dir = tmp_dir("decl_full_mirror");
    let info = dir.join("info");
    std::fs::create_dir_all(&info).unwrap();
    std::fs::write(info.join("TIANGONG.md"), "# 镜像全文").unwrap();
    let code = dir.join("code");
    std::fs::create_dir_all(&code).unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![pair(
            &code.to_string_lossy(),
            &info.to_string_lossy(),
            TIANGONG_LOAD_FULL,
        )],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.contains("全文注入"), "{section}");
    assert!(section.contains("# 镜像全文"), "{section}");
    assert!(section.contains(&info.join("TIANGONG.md").to_string_lossy().to_string()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_decl_multi_pair_same_name_layer_dedup() {
    // 多 pair 同名子目录层（各自镜像各有一份）→ 层合并判定：任意 pair
    // 命中即命中；括注一条（「根目录、src」），同名根层不重复出现。
    let dir = tmp_dir("decl_multi_pair");
    let code_a = dir.join("code_a");
    let code_b = dir.join("code_b");
    let info_a = dir.join("info_a");
    let info_b = dir.join("info_b");
    for d in [&code_a, &code_b] {
        std::fs::create_dir_all(d.join("src")).unwrap();
    }
    std::fs::create_dir_all(&info_b).unwrap();
    std::fs::create_dir_all(info_b.join("src")).unwrap();
    std::fs::write(info_b.join("TIANGONG.md"), "b root").unwrap();
    std::fs::write(info_b.join("src").join("TIANGONG.md"), "b mirror").unwrap();
    let cfg = KnowledgeConfig {
        enabled: true,
        pairs: vec![
            pair(
                &code_a.to_string_lossy(),
                &info_a.to_string_lossy(),
                TIANGONG_LOAD_POINTER,
            ),
            pair(
                &code_b.to_string_lossy(),
                &info_b.to_string_lossy(),
                TIANGONG_LOAD_POINTER,
            ),
        ],
    };
    let section = knowledge_decl_section(&cfg).unwrap();
    assert!(section.starts_with("# 天工"), "{section}");
    // 合并括注一条；同名根层去重后 parts 只出现一次「根目录」。
    assert!(section.contains("存在层级：根目录、src"), "{section}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- KnowledgeHandle ----------

#[test]
fn test_handle_missing_config_fail_closed() {
    let h = KnowledgeHandle::new(PathBuf::from("Z:/nonexistent/config.knowledge.json"));
    assert!(!h.get().enabled);
}

#[test]
fn test_handle_mtime_cache_rereads_on_change() {
    let dir = tmp_dir("handle_mtime");
    let path = dir.join("config.knowledge.json");
    std::fs::write(
        &path,
        r#"{"enabled":false,"pairs":[{"code_root":"C:/a","info_root":"C:/b","tiangong_load":"pointer"}]}"#,
    )
    .unwrap();
    let h = KnowledgeHandle::new(path.clone());
    assert!(!h.get().enabled, "初始关闭");

    // 改配置（含 mtime 变化）→ 下次 get 重读。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(
        &path,
        r#"{"enabled":true,"pairs":[{"code_root":"C:/a","info_root":"C:/b","tiangong_load":"pointer"}]}"#,
    )
    .unwrap();
    assert!(h.get().enabled, "mtime 变化后重读");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_handle_caches_unchanged_mtime() {
    let dir = tmp_dir("handle_cache");
    let path = dir.join("config.knowledge.json");
    std::fs::write(&path, r#"{"enabled":true,"pairs":[]}"#).unwrap();
    let h = KnowledgeHandle::new(path.clone());
    let first = h.get();
    let second = h.get();
    assert!(Arc::ptr_eq(&first, &second), "mtime 未变应命中缓存");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- is_instruction_file_name ----------

#[test]
fn test_is_instruction_file_name() {
    assert!(is_instruction_file_name("AGENTS.md"));
    assert!(is_instruction_file_name("CLAUDE.md"));
    assert!(is_instruction_file_name("TIANGONG.md"));
    assert!(!is_instruction_file_name("agents.md"));
    assert!(!is_instruction_file_name("README.md"));
    assert!(!is_instruction_file_name("main.rs"));
}

// ---------- fence_roots（第四轮横扫下沉的单一真相源） ----------

#[test]
fn test_fence_roots_disabled_is_empty() {
    let dir = tmp_dir("fence_disabled");
    let path = dir.join("config.knowledge.json");
    std::fs::write(&path, r#"{"enabled":false,"pairs":[{"code_root":"D:/a","info_root":"D:/b","tiangong_load":"pointer"}]}"#).unwrap();
    let h = KnowledgeHandle::new(path);
    assert!(
        h.fence_roots().is_empty(),
        "开关关 = 空集（功能不存在零放行）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_fence_roots_enabled_sorted_dedup_both_sides() {
    let dir = tmp_dir("fence_enabled");
    let path = dir.join("config.knowledge.json");
    // 乱序 + 多对共享目录（D:/shared 两侧重复出现）→ 排序去重双根
    std::fs::write(
        &path,
        r#"{"enabled":true,"pairs":[
            {"code_root":"D:/z/proj","info_root":"D:/a/info","tiangong_load":"pointer"},
            {"code_root":"D:/shared","info_root":"D:/shared","tiangong_load":"pointer"}]}"#,
    )
    .unwrap();
    let h = KnowledgeHandle::new(path);
    let roots = h.fence_roots();
    // 第五轮横扫 2026-11-27：Windows 上根经 normalize_root_separators
    // 归一为反斜杠（Sandboxie ini 字面前缀匹配纪律），期望值分平台。
    let expected: Vec<PathBuf> = ["D:/a/info", "D:/shared", "D:/z/proj"]
        .iter()
        .map(|s| {
            PathBuf::from(if cfg!(windows) {
                s.replace('/', "\\")
            } else {
                (*s).to_string()
            })
        })
        .collect();
    assert_eq!(
        roots, expected,
        "双根排序去重（防 read_dir 顺序语义 + 共享目录重复放行）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg(windows)]
fn test_fence_roots_windows_forward_slash_normalized() {
    // 第五轮横扫 2026-11-27：用户 JSON 配置正斜杠形态（前端占位示例即
    // 此形态）→ fence_roots 输出反斜杠（OpenFilePath 字面前缀匹配纪律
    // ——混合分隔符行匹配语义未验证，从紧归一）。
    let dir = tmp_dir("fence_norm");
    let path = dir.join("config.knowledge.json");
    std::fs::write(
        &path,
        r#"{"enabled":true,"pairs":[{"code_root":"D:/kb/code","info_root":"D:/kb/info","tiangong_load":"pointer"}]}"#,
    )
    .unwrap();
    let h = KnowledgeHandle::new(path);
    let roots = h.fence_roots();
    assert_eq!(
        roots,
        vec![PathBuf::from("D:\\kb\\code"), PathBuf::from("D:\\kb\\info")],
    );
    for r in &roots {
        assert!(
            !r.to_string_lossy().contains('/'),
            "围栏根残留正斜杠: {r:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_fence_roots_missing_config_fail_closed() {
    let h = KnowledgeHandle::new(PathBuf::from("Z:/nonexistent/config.knowledge.json"));
    assert!(h.fence_roots().is_empty(), "缺席配置 = 关闭态 = 空集");
}

// ---------- global_write_through_roots（第八轮横扫 2026-11-27） ----------

/// 全景清单辅助：构造 workspace 级 + agent 级配置布局（json 空串 = 该级
/// 不落配置文件——agent 目录在场但无知识库配置的生命周期常态）。
fn seed_global_ws(dir: &Path, workspace_cfg: Option<&str>, agents: &[(&str, &str)]) {
    if let Some(json) = workspace_cfg {
        let cfg_dir = dir.join("config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(cfg_dir.join("config.knowledge.json"), json).unwrap();
    }
    for (id, json) in agents {
        let agent_cfg = dir.join("agents").join(id).join("home").join("config");
        std::fs::create_dir_all(&agent_cfg).unwrap();
        if !json.is_empty() {
            std::fs::write(agent_cfg.join("config.knowledge.json"), json).unwrap();
        }
    }
}

fn expected_roots(paths: &[&str]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = paths
        .iter()
        .map(|s| {
            PathBuf::from(if cfg!(windows) {
                s.replace('/', "\\")
            } else {
                (*s).to_string()
            })
        })
        .collect();
    v.sort();
    v
}

#[test]
fn test_global_write_through_roots_merges_workspace_and_agents() {
    let dir = tmp_dir("global_merge");
    seed_global_ws(
        &dir,
        Some(
            r#"{"enabled":true,"pairs":[{"code_root":"D:/ws/proj","info_root":"D:/ws/info","tiangong_load":"pointer"}]}"#,
        ),
        &[
            (
                "ag-1",
                r#"{"enabled":true,"pairs":[{"code_root":"D:/ag1/proj","info_root":"D:/ag1/info","tiangong_load":"pointer"}]}"#,
            ),
            // ag-2 目录在但无知识库配置（agent 生命周期常态）→ 零贡献。
            ("ag-2", ""),
        ],
    );

    let roots = global_write_through_roots(&dir);
    assert_eq!(
        roots,
        expected_roots(&["D:/ws/info", "D:/ws/proj", "D:/ag1/info", "D:/ag1/proj"]),
        "workspace 级 + agent 级双根并集排序"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_global_write_through_roots_absent_everything_is_empty() {
    let dir = tmp_dir("global_absent");
    // 无 workspace 级配置、无 agents 目录 → 空集（全局盒 ini 回落 start 形态）。
    assert!(global_write_through_roots(&dir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_global_write_through_roots_agent_disabled_no_contribution() {
    let dir = tmp_dir("global_disabled");
    seed_global_ws(
        &dir,
        Some(
            r#"{"enabled":true,"pairs":[{"code_root":"D:/ws/proj","info_root":"D:/ws/info","tiangong_load":"pointer"}]}"#,
        ),
        &[(
            "ag-off",
            r#"{"enabled":false,"pairs":[{"code_root":"D:/off/proj","info_root":"D:/off/info","tiangong_load":"pointer"}]}"#,
        )],
    );
    let roots = global_write_through_roots(&dir);
    assert_eq!(
        roots,
        expected_roots(&["D:/ws/info", "D:/ws/proj"]),
        "agent 级 disabled = 零贡献（其工具不重定向，无镜像写可穿透）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_global_write_through_roots_dedup_across_levels() {
    let dir = tmp_dir("global_dedup");
    let shared = r#"{"enabled":true,"pairs":[{"code_root":"D:/shared/proj","info_root":"D:/shared/info","tiangong_load":"pointer"}]}"#;
    seed_global_ws(&dir, Some(shared), &[("ag-same", shared)]);
    let roots = global_write_through_roots(&dir);
    assert_eq!(
        roots,
        expected_roots(&["D:/shared/info", "D:/shared/proj"]),
        "两级共享同一 pair → 排序去重单次"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
