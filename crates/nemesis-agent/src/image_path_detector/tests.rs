//! image_path_detector 单测（goal T4 验证清单）：
//! 中文/空格/引号路径、UNC、多路径去重、工作区相对、
//! 不存在/超限/magic 失败原因、普通文本不误判。

use super::*;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

/// 每测试独立临时目录（进程唯一后缀防并行冲突）。
fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nbdet-{}-{}-{}", tag, std::process::id(), nanos));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 写一个最小合法 PNG（8 字节签名 + IHDR 长度占位）。
fn write_png(path: &Path, extra: &[u8]) -> u64 {
    let mut data = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    data.extend_from_slice(extra);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, &data).unwrap();
    data.len() as u64
}

/// 写一个伪装 .png 的纯文本文件。
fn write_fake_png(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, b"this is not an image at all, just text").unwrap();
}

// ============================================================
// 提取形态
// ============================================================

#[test]
fn test_windows_absolute_path_detected() {
    let ws = temp_dir("win-abs");
    let img = ws.join("a.png");
    write_png(&img, b"x");

    // 文件在 workspace 下，但消息里写的是绝对路径
    let text = format!("看看 {} 有什么问题", img.to_string_lossy());
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1);
    assert!(out[0].is_attachable());
    assert!(out[0].deliberate);
    assert_eq!(out[0].resolved, img);
}

#[test]
fn test_windows_path_forward_slash_mix() {
    let ws = temp_dir("mix-sep");
    write_png(&ws.join("sub").join("b.jpg"), b"x");

    // 正斜杠写法在 Windows 上同样有效
    let text = format!("看 {}/sub/b.jpg", ws.to_string_lossy().replace('\\', "/"));
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1, "正斜杠绝对路径应被检出: {:?}", out);
    assert!(out[0].is_attachable());
}

// UNC 是 Windows 形态语义（`\` 是分隔符、`\\server` 是根）：POSIX 上
// `Path::has_root()` 对反斜杠前缀返回 false → 按相对路径处理 → 本测试的
// 「deliberate + 诚实 NotFound」前提不成立。2026-09-02 约定：Windows 形态
// 测试挂 #[cfg(windows)]，Linux 上编译期消失而非运行期跳过（2026-09-04
// Linux CI 红 9 处的根因之一）。
#[test]
#[cfg(windows)]
fn test_unc_path_detected() {
    let ws = temp_dir("unc");
    // UNC 无法在单机测试里真实创建共享；验证提取 + 诚实的 NotFound
    let text = r"帮我看 \\server\share\photos\风景.png 这张图";
    let out = detect_image_paths(text, Some(&ws));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].raw, r"\\server\share\photos\风景.png");
    assert!(out[0].deliberate);
    // 单机无该共享 → NotFound（deliberate → failure_reason 有内容）
    assert_eq!(out[0].status, CandidateStatus::NotFound);
    assert!(out[0].failure_reason().is_some());
}

#[test]
fn test_workspace_relative_path_resolves() {
    let ws = temp_dir("rel");
    write_png(&ws.join("images").join("屏幕截图.png"), b"x");

    let out = detect_image_paths("看 images/屏幕截图.png", Some(&ws));
    assert_eq!(out.len(), 1);
    assert!(out[0].is_attachable());
    assert_eq!(out[0].resolved, ws.join("images").join("屏幕截图.png"));
    assert!(!out[0].deliberate);
}

#[test]
fn test_dot_relative_path_resolves() {
    let ws = temp_dir("dot-rel");
    write_png(&ws.join("c.webp"), b"x");

    let out = detect_image_paths("./c.webp 在哪", Some(&ws));
    assert_eq!(out.len(), 1);
    assert!(out[0].is_attachable());
}

#[test]
fn test_posix_absolute_path_form_detected() {
    // POSIX 形态在 Windows 上通常不存在 → 诚实 NotFound（deliberate）
    let ws = temp_dir("posix");
    let out = detect_image_paths("看 /home/zoo/shots/d.png 谢谢", Some(&ws));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].raw, "/home/zoo/shots/d.png");
    assert!(out[0].deliberate);
    assert_eq!(out[0].status, CandidateStatus::NotFound);
}

// ============================================================
// 误报防线
// ============================================================

#[test]
fn test_plain_text_not_misdetected() {
    let ws = temp_dir("plain");
    for text in [
        "png 是一种图片格式",
        "jpg 和 jpeg 有什么区别？",
        "我的头像.gif 很搞笑",
        "把图片转成 webp 最好",
    ] {
        let out = detect_image_paths(text, Some(&ws));
        assert!(out.is_empty(), "普通文本不应误判: {:?} → {:?}", text, out);
    }
}

#[test]
fn test_bare_filename_not_candidate() {
    let ws = temp_dir("bare");
    write_png(&ws.join("photo.png"), b"x");
    // 裸文件名（无分隔符）不附加——即使 workspace 里恰好存在同名文件
    let out = detect_image_paths("打开 photo.png", Some(&ws));
    assert!(out.is_empty(), "裸文件名不应成为候选: {:?}", out);
}

#[test]
fn test_nonexistent_relative_candidate_dropped_silently() {
    let ws = temp_dir("rel-miss");
    // 行文里的斜杠词 "and/or.png" 不存在 → 静默丢弃，不产噪
    let out = detect_image_paths("this and/or.png thing", Some(&ws));
    assert!(out.is_empty(), "不存在且非点名的候选应静默: {:?}", out);
}

#[test]
fn test_nonexistent_deliberate_path_reports_honestly() {
    let ws = temp_dir("abs-miss");
    let missing = ws.join("nope").join("ghost.png");
    let text = format!("看看 {}", missing.to_string_lossy());
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].status, CandidateStatus::NotFound);
    let reason = out[0].failure_reason().unwrap();
    assert!(reason.contains("文件不存在"), "reason = {}", reason);
    assert!(reason.contains("ghost.png"));
}

// ============================================================
// 验真
// ============================================================

#[test]
fn test_fake_png_extension_spoof_detected() {
    let ws = temp_dir("spoof");
    let p = ws.join("fake.png");
    write_fake_png(&p);

    let text = format!("看 {}", p.to_string_lossy());
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1);
    match &out[0].status {
        CandidateStatus::NotImage { magic_hex } => {
            assert_eq!(magic_hex, "74 68 69 73", "应为文本字节 this")
        }
        other => panic!("应识别为扩展名伪装，got {:?}", other),
    }
    assert!(out[0].failure_reason().unwrap().contains("伪装"));
}

#[test]
fn test_oversize_image_rejected() {
    let ws = temp_dir("big");
    let p = ws.join("huge.png");
    // 超 25MB：写稀疏式大文件代价高 → 直接造 25MB+1 的 png 头文件
    let mut data = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    data.resize(super::MAX_IMAGE_BYTES as usize + 1, 0);
    fs::write(&p, &data).unwrap();

    let text = format!("看 {}", p.to_string_lossy());
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1);
    match &out[0].status {
        CandidateStatus::TooLarge { size } => {
            assert_eq!(*size, super::MAX_IMAGE_BYTES + 1)
        }
        other => panic!("应超限，got {:?}", other),
    }
}

#[test]
fn test_directory_named_like_image_not_candidate() {
    let ws = temp_dir("dir");
    fs::create_dir_all(ws.join("pics.png")).unwrap();
    let text = format!("看 {}", ws.join("pics.png").to_string_lossy());
    let out = detect_image_paths(&text, Some(&ws));
    // 目录不是文件 → NotFound（deliberate）诚实注明
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].status, CandidateStatus::NotFound);
}

// ============================================================
// 去重
// ============================================================

#[test]
fn test_duplicate_paths_deduped() {
    let ws = temp_dir("dedup");
    let img = ws.join("d2.png");
    write_png(&img, b"x");

    let raw = img.to_string_lossy().to_string();
    let text = format!("看 {} 和 {} 还有 {}", raw, raw, raw);
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1, "同消息重复路径应去重: {:?}", out);
}

// Windows 文件系统语义（大小写不敏感去重）——cfg(windows)：非 Windows 平台
// `A.png` 与 `a.png` 是两个不同文件（dedup_key 按 cfg!(windows) 小写化），
// 本测试的「变体应去重」前提只在 Windows 成立（2026-09-03 二次回归 C2；
// 2026-09-02 约定：Windows 形态测试挂标记，编译期消失而非运行期跳过）。
#[test]
#[cfg(windows)]
fn test_case_insensitive_dedup_on_windows() {
    let ws = temp_dir("case");
    let img = ws.join("Case.png");
    write_png(&img, b"x");

    let lower = img.to_string_lossy().to_lowercase();
    let text = format!("看 {} 和 {}", img.to_string_lossy(), lower);
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1, "大小写变体应去重: {:?}", out);
}

// 2026-09-03 二次回归 C1：`\` 与 `/` 混用路径必须去重为同一张（检测器明确
// 接受混用；旧实现 raw 小写化不分隔符归一 → 同图重复附加）。
#[test]
fn test_mixed_separator_paths_deduped() {
    let ws = temp_dir("seps");
    let img = ws.join("sep.png");
    write_png(&img, b"x");

    // 只换第一个分隔符 → 真「混用」形态（Windows：`C:/Users\...\sep.png`；
    // POSIX 路径无反斜杠时退化为同形字符串，仍过恒等去重）。
    let raw = img.to_string_lossy().to_string();
    let mixed = raw.replacen('\\', "/", 1);
    let text = format!("看 {} 和 {}", raw, mixed);
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1, "分隔符变体应去重: {:?}", out);
}

// 2026-09-03 二次回归 C3：ext_from_magic 与 sniff_magic 同签名表（sniff ==
// ext.is_some），且各签名映射到正确扩展名（URL 落盘按内容定扩展名）。
#[test]
fn test_ext_from_magic_matches_sniff_and_names() {
    let cases: Vec<(&[u8], &str)> = vec![
        (&[0x89, b'P', b'N', b'G', 0x0D, 0x0A], "png"),
        (&[0xFF, 0xD8, 0xFF, 0xE0], "jpg"),
        (b"GIF89a", "gif"),
        (b"RIFF\x00\x00\x00\x00WEBPVP8 ", "webp"),
    ];
    for (bytes, ext) in cases {
        assert!(super::sniff_magic(bytes), "sniff 应识别 {:?}", ext);
        assert_eq!(super::ext_from_magic(bytes), Some(ext));
    }
    // 非图片 / 不足长 / WebP 截断（RIFF 无 WEBP 标记）→ None 且 sniff false。
    for bytes in [&b"hello world"[..], &[0x00u8, 0x01, 0x02][..], b"RIFF____"] {
        assert!(!super::sniff_magic(bytes));
        assert_eq!(super::ext_from_magic(bytes), None);
    }
}

// 2026-09-03 二次回归 C1：dedup_key 分隔符归一。**Windows 形态语义**——
// 归一与小写化都只 cfg!(windows)（L5：POSIX 上 `\` 是合法文件名字符），
// POSIX 侧的「不归一」前提由 test_posix_backslash_filename_not_deduped
// 锁定。挂 #[cfg(windows)]（2026-09-02 约定；2026-09-04 Linux CI 红 9 处
// 的根因之一）。
#[test]
#[cfg(windows)]
fn test_dedup_key_separator_normalized() {
    let a = super::dedup_key(std::path::Path::new("C:\\imgs\\A.png"));
    let b = super::dedup_key(std::path::Path::new("C:/imgs/A.png"));
    assert_eq!(a, b, "分隔符变体应为同一去重键");
    assert!(!a.contains('\\'), "归一后不应残留反斜杠");
}

// L5（2026-09-04 四轮盲审）：分隔符归一仅 Windows——POSIX 上 `\` 是合法
// 文件名字符，`dir\a.png` 与 `dir/a.png` 是两个不同文件，归一会把同消息里
// 的两个不同文件错误去重掉一个。
#[cfg(not(windows))]
#[test]
fn test_posix_backslash_filename_not_deduped() {
    let ws = temp_dir("posixbs");
    // POSIX：字面反斜杠文件名（合法）。
    let a = ws.join("dir\\a.png");
    let b = ws.join("dir").join("a.png");
    std::fs::create_dir_all(a.parent().unwrap()).unwrap();
    std::fs::create_dir_all(b.parent().unwrap()).unwrap();
    write_png(&a, b"x");
    write_png(&b, b"y");

    let text = "对比 dir\\a.png 和 dir/a.png";
    let out = detect_image_paths(text, Some(&ws));
    assert_eq!(out.len(), 2, "POSIX 反斜杠文件名不应被归一去重: {:?}", out);
}

// F-H（2026-09-04 四轮盲审）：URL 内的路径形态不是文本附加候选——
// `https://host/img/a.png` 的 `/host/img/a.png` 会被 nix_abs（无左边界）
// 抓成"POSIX 绝对路径"（假 deliberate → 每次灌一条假"文件不存在"注记；
// 碰巧存在时错误附加本地文件）。URL 附加走 media 引用预取（T9），文本
// 提及不产出候选。
#[test]
fn test_url_text_mentions_not_candidates() {
    let ws = temp_dir("urlspan");
    let img = ws.join("real.png");
    write_png(&img, b"x");

    let out = detect_image_paths("看 https://example.com/img/photo.png 这张", Some(&ws));
    assert!(out.is_empty(), "URL 内路径形态不应产出候选: {:?}", out);

    // 同消息混排：真实本地路径照常检出，URL 部分被排除。
    let text = format!(
        "对比 https://example.com/img/photo.png 和 {}",
        img.to_string_lossy()
    );
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1, "只有本地路径应检出: {:?}", out);
    assert_eq!(out[0].resolved, img);
}

// ============================================================
// 引号与多路径
// ============================================================

#[test]
fn test_quoted_path_extracted() {
    let ws = temp_dir("quoted");
    let img = ws.join("my photo.png");
    write_png(&img, b"x");

    // 双引号包裹（含空格路径）
    let text = format!("看下 \"{}\" 这张", img.to_string_lossy());
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1);
    assert!(out[0].is_attachable());
    assert_eq!(out[0].resolved, img);

    // 中文引号「」包裹同理（引号不在段字符类内，天然剥离）
    let text2 = format!("看下「{}」这张", img.to_string_lossy());
    let out2 = detect_image_paths(&text2, Some(&ws));
    assert_eq!(out2.len(), 1);
    assert!(out2[0].is_attachable());
}

#[test]
fn test_multiple_paths_all_detected() {
    let ws = temp_dir("multi");
    let a = ws.join("a1.png");
    let b = ws.join("b2.jpg");
    write_png(&a, b"1");
    // b2 用 jpeg 魔数
    fs::write(&b, [0xFF, 0xD8, 0xFF, 0xE0]).unwrap();

    let text = format!(
        "对比 {} 和 {} 的差别",
        a.to_string_lossy(),
        b.to_string_lossy()
    );
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 2, "两条不同路径都应检出: {:?}", out);
    assert!(out.iter().all(|c| c.is_attachable()));
}

#[test]
fn test_no_workspace_dir_relative_dropped() {
    // 无 workspace_dir 基准时相对候选不产出（避免误报）
    let out = detect_image_paths("看 sub/dir/a.png", None);
    assert!(out.is_empty());
}

// ============================================================
// 2026-09-04 CI 回归根修回归锁（28 处红 → 正则两形态家族重构）
// ============================================================

// `~`（8.3 短名）回归：段字符类此前不含 `~`，`C:\Users\RUNNER~1\...`
// （GitHub Actions Windows runner 的 std::env::temp_dir() 真实返回形态，
// 开启 8.3 短名生成的机器同理）永不匹配 → 文本点名路径全部失明（Windows
// CI 红 19 处）。`~` 在 Windows/POSIX 文件名里都合法，用字面 RUNNER~1
// 目录名做双平台确定性回归。
#[test]
fn test_83_shortname_tilde_segment_detected() {
    let base = temp_dir("tilde");
    let dir = base.join("RUNNER~1");
    let img = dir.join("a.png");
    write_png(&img, b"x");

    let text = format!("看 {}", img.to_string_lossy());
    let out = detect_image_paths(&text, Some(&base));
    assert_eq!(out.len(), 1, "含 ~ 的 8.3 短名路径应被检出: {:?}", out);
    assert!(out[0].is_attachable());
    assert_eq!(out[0].resolved, img);
}

// 贪心粘连回归：段字符类含空格时，无锚点的 POSIX/相对分支把
// 「/a.png 和 /b.png」连成**一个**候选（find_iter 只取这个重叠匹配，
// 真路径全丢）。Windows 靠盘符 `:` 不在类内天然截断所以本地测不出；
// Linux CI 上是生产 bug（同句两图粘连成假 NotFound，红 9 处之一）。
// 空白出类后同句多路径各自成候选。POSIX 形态在 Windows 上按 NotFound
// deliberate 检出（形态 bug 与操作系统无关），故双平台可跑。
#[test]
fn test_two_paths_in_one_sentence_not_joined() {
    let ws = temp_dir("nojoin");
    let out = detect_image_paths("对比 /tmp/x/a1.png 和 /tmp/x/b2.jpg 的差别", Some(&ws));
    assert_eq!(out.len(), 2, "同句两个 POSIX 路径不应粘连: {:?}", out);
    assert_eq!(out[0].raw, "/tmp/x/a1.png");
    assert_eq!(out[1].raw, "/tmp/x/b2.jpg");
    assert!(out.iter().all(|c| c.deliberate));
}

// 引号形态覆盖**中间段**空格（目录名含空格，如 "my dir"）：含空格路径
// 必须加引号（shell 语义），引号内任意段可含空格。
#[test]
fn test_quoted_path_with_space_dir_detected() {
    let ws = temp_dir("quotedir");
    let img = ws.join("my dir").join("a.png");
    write_png(&img, b"x");

    let text = format!("看下 \"{}\" 这张", img.to_string_lossy());
    let out = detect_image_paths(&text, Some(&ws));
    assert_eq!(out.len(), 1, "引号包裹的含空格目录路径应被检出: {:?}", out);
    assert!(out[0].is_attachable());
    assert_eq!(out[0].resolved, img);
}

// 行为决策锁：无引号含空格路径**不**检出（空白 = 行文边界，shell 语义；
// 加引号即可检出，见 test_quoted_path_with_space_dir_detected）。这是
// 贪心粘连根修的代价面——若要恢复无引号空格支持，必须先解决同句多路径
// 粘连，不能简单把空格加回段字符类（CI 会重新红给你看）。
#[test]
fn test_unquoted_space_path_not_candidate() {
    let ws = temp_dir("nospace");
    let img = ws.join("my photo.png");
    write_png(&img, b"x");

    let text = format!("看 {}", img.to_string_lossy());
    let out = detect_image_paths(&text, Some(&ws));
    assert!(
        out.is_empty(),
        "无引号含空格路径不检出（shell 语义决策锁）: {:?}",
        out
    );
}
