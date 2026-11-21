//! 契约 drift 测试——C-header 模式副本与宿主真相源的 ABI 布局相等。
//!
//! 本插件的 host_services.rs 是「仅结构定义」副本（副本含 plugin 侧
//! helper、无宿主实现——与 plugin-mail/mail.rs 的全文件复制不同），同步
//! 单位是 **ABI 布局**（`#[repr(C)]` 属性 + 字段序 + 字段类型），不是注释
//! 与 helper。因此守卫形态 = 把 `#[repr(C)]` 结构块归一化（剥注释行与
//! 空白）后比对，而非全文件字节相等。
//!
//! 教训（BUG-20261121-002）：真相源 host_services.rs 的 decode_png doc
//! 曾被单独改坏（`-1=参数无效` 丢负号成 `1=参数无效`，与宿主实现返回
//! `-1` 矛盾），「Keep in sync」手工纪律无自动化守卫即静默失效。

/// 提取 `#[repr(C)] pub struct <name> { … }` 块并归一化：从结构体声明前
/// 最近的 `#[repr(C)]` 起，剥注释行/空行/行首尾空白后逐行收集，花括号
/// 配对取块尾。注释先剥再计数——doc 注释里的花括号不干扰配对。
fn abi_block(text: &str, struct_name: &str) -> String {
    let marker = format!("pub struct {} {{", struct_name);
    let decl_pos = text
        .find(&marker)
        .unwrap_or_else(|| panic!("缺结构体定义：{}", struct_name));
    let attr_pos = text[..decl_pos]
        .rfind("#[repr(C)]")
        .unwrap_or_else(|| panic!("{} 缺 #[repr(C)] 属性", struct_name));
    let lines: Vec<&str> = text[attr_pos..]
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .collect();
    let mut depth = 0usize;
    let mut end = None;
    for (i, line) in lines.iter().enumerate() {
        for b in line.bytes() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        if end.is_some() {
            break;
        }
    }
    let end = end.unwrap_or_else(|| panic!("{} 结构块未闭合", struct_name));
    lines[..=end].join("\n")
}

#[test]
fn host_services_abi_layout_matches_truth_source() {
    let mine = include_str!("host_services.rs");
    let host = include_str!("../../../crates/nemesis-plugin/src/host_services.rs");
    assert_eq!(
        abi_block(mine, "HostServices"),
        abi_block(host, "HostServices"),
        "HostServices ABI 布局与宿主真相源漂移（字段序/类型/repr 属性）——修改必须双侧同步"
    );
}
