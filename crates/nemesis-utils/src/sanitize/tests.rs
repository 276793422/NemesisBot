use super::{
    escape_closing_tag, escape_like, redact_url_secrets, sanitize_csv, sanitize_path_segment,
};

#[test]
fn plain_ids_pass_through() {
    assert_eq!(
        sanitize_path_segment("agent_main_session_123"),
        "agent_main_session_123"
    );
    assert_eq!(
        sanitize_path_segment("task-1726000000000_ab12"),
        "task-1726000000000_ab12"
    );
    assert_eq!(
        sanitize_path_segment("node-laptop-a1b2"),
        "node-laptop-a1b2"
    );
}

#[test]
fn colons_and_separators_collapse_to_underscore() {
    // 旧 `replace(':', "_")` 语义保留：复合键文件名与存量平面文件一致。
    assert_eq!(
        sanitize_path_segment("agent:main:session:1726"),
        "agent_main_session_1726"
    );
    assert_eq!(sanitize_path_segment("a/b\\c:d"), "a_b_c_d");
}

#[test]
fn slash_composite_key_flattens_instead_of_nesting() {
    // SAN-01 核心场景：B 端 `{node}/{chat}` 键不再拆出中间目录。
    assert_eq!(sanitize_path_segment("node-x/chat:123"), "node-x_chat_123");
}

#[test]
fn dot_guard_blocks_traversal_forms() {
    // SAN-03：`..` 原样放行会逃一级目录。
    assert_eq!(sanitize_path_segment(".."), "__");
    assert_eq!(sanitize_path_segment("..."), "___");
    assert_eq!(sanitize_path_segment("a..b"), "a._b"); // 首个 `.` 跟 alnum 保留
    assert_eq!(sanitize_path_segment(".hidden"), "_hidden");
    // 尾点修剪（Windows 文件名剥尾点，读侧会失配）。
    assert_eq!(sanitize_path_segment("a."), "a");
}

#[test]
fn empty_becomes_underscore() {
    assert_eq!(sanitize_path_segment(""), "_");
}

#[test]
fn length_capped_at_80_with_hash_suffix() {
    let long = sanitize_path_segment(&"a".repeat(120));
    assert_eq!(long.chars().count(), 78, "60 头 + 2 分隔 + 16 hex");
    assert!(long.starts_with(&"a".repeat(60)));
    assert!(long.contains("__"));
    // 截断留下的尾点被修剪（Windows 文件名剥尾点）；hash 后缀是 hex，
    // 超限形态永不触点。
    let dots = sanitize_path_segment(&format!("{}.", "a".repeat(90)));
    assert_eq!(dots.chars().count(), 78);
    assert!(!dots.ends_with('.'));
}

#[test]
fn over_limit_keys_differing_at_tail_do_not_collide() {
    // UAT 回归钉（改真实形态重钉）：cluster 会话键
    // `cluster_rpc:{node_id}/board:{n}`——派发 chat 形态恒为裸数字单号
    // （`board:NB-` 从未被生产代码产出，原夹具误用 issue key 形态）。
    // 节点 id 随主机名变长（此处 60 字符形态真实存在），两位数单号即把
    // 全长推到 81——纯截断使 10…19 仅末字符不同却双双落同名 `…board_1`，
    // 磁盘 fallback 跨会话串读。修复后必须各得唯一文件名。
    let node = "cluster_rpc:node-laptop-mbrq578qxyz-b11dbf4f-905f-40d9-8530-a2238e5aaf5c";
    let a = sanitize_path_segment(&format!("{node}/board:10"));
    let b = sanitize_path_segment(&format!("{node}/board:11"));
    let c = sanitize_path_segment(&format!("{node}/board:12"));
    assert_eq!(format!("{node}/board:10").chars().count(), 81);
    assert_ne!(a, b);
    assert_ne!(b, c);
    assert_ne!(a, c);
    // 同输入两次调用一致（读写两侧同源）。
    assert_eq!(a, sanitize_path_segment(&format!("{node}/board:10")));
    // 域分离：hash 形态产物再消毒**不再**原样（hash 输出形态是
    // 输出域专属——若直通原样，任何能注入会话键的一方都能把 78 字符
    // 撞形 id 直接落到别人会话的文件段上）。映射仍确定（读写两侧同函
    // 数同输入恒同输出），只是 hash 形态段不再幂等。
    assert_ne!(sanitize_path_segment(&a), a);
}

#[test]
fn keys_at_or_under_80_stay_byte_identical() {
    // 恰 80 字符（72 前缀 + 个位数单号）走原路径，逐字节不变——存量文件
    // 不失联。
    let node = "cluster_rpc:node-laptop-mbrq578qxyz-b11dbf4f-905f-40d9-8530-a2238e5aaf5c";
    let k80 = format!("{node}/board:9"); // 72 + 8 = 80
    assert_eq!(k80.chars().count(), 80);
    assert_eq!(
        sanitize_path_segment(&k80),
        "cluster_rpc_node-laptop-mbrq578qxyz-b11dbf4f-905f-40d9-8530-a2238e5aaf5c_board_9"
    );
}

#[test]
fn whitelist_is_idempotent() {
    // 读侧对已消毒 stem 再消毒必须原样（logs.rs ledger_file_name 语义）。
    let once = sanitize_path_segment("web://user/chat|file?x");
    assert_eq!(sanitize_path_segment(&once), once);
    assert_eq!(once, "web___user_chat_file_x");
}

// ---------- （第 6 轮审计）：hash 输出形态与输入域分离 ----------

#[test]
fn hash_form_lookalike_input_does_not_pass_through() {
    // 78 字符、形如 `<60>__<16hex>` 的合法输入曾逐字节直通——与超限输入
    // 的 hash 输出同形态，撞形 id 直接落到目标会话的文件段（确定性伪造，
    // 无需哈希碰撞）。域分离后必须改道 hash 分支。
    let lookalike = format!("{}__0123456789abcdef", "a".repeat(60));
    assert_eq!(lookalike.len(), 78);
    let out = sanitize_path_segment(&lookalike);
    assert_ne!(out, lookalike, "撞形输入不得直通");
    assert_eq!(out.len(), 78, "改道后仍是 hash 形态");
    assert!(out.starts_with(&"a".repeat(60)));
    // 尾点修剪后再撞形同样改道（`<60>__<16hex>.` 修剪后恰是 hash 形态）。
    let dotted = format!("{lookalike}.");
    assert_ne!(sanitize_path_segment(&dotted), dotted.trim_end_matches('.'));
    // 接近但不构成 hash 形态的输入照常直通（不扩大改道面）：
    // 77 字符（hex 少一位）不撞形。
    let short = format!("{}__0123456789abcde", "a".repeat(60));
    assert_eq!(sanitize_path_segment(&short), short);
    // 大写 hex 不是 hash 分支产出形态（输出恒小写），不撞形。
    let upper = format!("{}__0123456789ABCDEF", "a".repeat(60));
    assert_eq!(sanitize_path_segment(&upper), upper);
}

#[test]
fn hash_output_represented_as_id_lands_elsewhere() {
    // 攻击面回归钉：受害者超限会话键的文件段 V，作为**原始 id**再喂入
    // 不得落回 V（纯截断碰撞根修的完整性补丁）。
    let node = "cluster_rpc:node-laptop-mbrq578qxyz-b11dbf4f-905f-40d9-8530-a2238e5aaf5c";
    let victim = sanitize_path_segment(&format!("{node}/board:10"));
    assert_eq!(victim.len(), 78);
    let relay = sanitize_path_segment(&victim);
    assert_ne!(relay, victim, "hash 段作原始 id 不得落回本段");
    // 确定性：同一输入恒同输出（读写两侧同函数）。
    assert_eq!(sanitize_path_segment(&victim), relay);
}

#[test]
fn csv_plain_field_passes_through_unquoted() {
    // 普通字段逐字节直通——档案 usage.csv 现网行形态不受影响（无谓引号
    // 会让既有消费者误判）。
    assert_eq!(sanitize_csv("task-a1"), "task-a1");
    assert_eq!(sanitize_csv("node-w"), "node-w");
    assert_eq!(sanitize_csv(""), "");
    assert_eq!(sanitize_csv("节点 甲"), "节点 甲");
}

#[test]
fn csv_special_chars_wrap_and_double_inner_quotes() {
    assert_eq!(sanitize_csv("a,b"), "\"a,b\"");
    assert_eq!(sanitize_csv("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(sanitize_csv("a,\"b\",c"), "\"a,\"\"b\"\",c\"");
    assert_eq!(sanitize_csv("line1\nline2"), "\"line1\nline2\"");
    // `\r` 也触发包裹（RFC 4180 须引号字符；比 audit_log 旧版多认）。
    assert_eq!(sanitize_csv("cr\rtail"), "\"cr\rtail\"");
}

// ----------：LIKE 通配符字面化（统一定义出口） ----------

#[test]
fn escape_like_escapes_wildcards_and_backslash_first() {
    // 三个通配/转义字符逐一（配 SQL `ESCAPE '\'` 子句消费）。
    assert_eq!(escape_like("a%b"), "a\\%b");
    assert_eq!(escape_like("a_b"), "a\\_b");
    // `\` 必须最先转义（防二次解义：若先转义 `%`，已有 `\%` 里的 `\`
    // 会在被翻倍，语义漂移）。
    assert_eq!(escape_like("a\\b"), "a\\\\b");
    // 普通词直通。
    assert_eq!(escape_like("glm-4.7"), "glm-4.7");
    assert_eq!(escape_like(""), "");
    // 混合：一次覆盖三字符 + 无关字符。
    assert_eq!(escape_like("50_100\\200%"), "50\\_100\\\\200\\%");
}

// ----------：闭合标签逃逸（ASCII 大小写折叠，统一实现出口） ----------

#[test]
fn escape_closing_tag_case_fold_and_preserve() {
    // 精确形态：`<` 后插 `\`，标签本体保留原样。
    assert_eq!(
        escape_closing_tag("a</system-reminder>b", "system-reminder"),
        "a<\\/system-reminder>b"
    );
    // 大小写变体同样命中（模型解析 HTML 形标记时大小写不敏感）。
    assert_eq!(
        escape_closing_tag("</SYSTEM-REMINDER>", "system-reminder"),
        "<\\/SYSTEM-REMINDER>"
    );
    assert_eq!(
        escape_closing_tag("</System-Reminder>", "system-reminder"),
        "<\\/System-Reminder>"
    );
    // 原始大小写形态保留（只插转义符，不改写内容）。
    assert_eq!(
        escape_closing_tag("</UnTrUsTeD_CoNtEnT>", "untrusted_content"),
        "<\\/UnTrUsTeD_CoNtEnT>"
    );
    // 多次出现全部处理。
    assert_eq!(escape_closing_tag("</t></t>", "t"), "<\\/t><\\/t>");
    // 无命中直通（含开标签——开标签不转义，与 untrusted_content 防御同口径）。
    assert_eq!(
        escape_closing_tag("<system-reminder>x", "system-reminder"),
        "<system-reminder>x"
    );
    assert_eq!(
        escape_closing_tag("benign text", "system-reminder"),
        "benign text"
    );
    // 近邻形态不误伤（缺 `>` / 多空格 / 前缀延长）。
    assert_eq!(
        escape_closing_tag("</system-reminder", "system-reminder"),
        "</system-reminder"
    );
    assert_eq!(
        escape_closing_tag("</ system-reminder>", "system-reminder"),
        "</ system-reminder>"
    );
    assert_eq!(
        escape_closing_tag("</system-reminderx>", "system-reminder"),
        "</system-reminderx>"
    );
    // 非 ASCII 内容偏移安全（İ 展开两字节不破坏扫描）。
    let unicode_mixed = "\u{0130}stanbul</system-reminder>";
    assert_eq!(
        escape_closing_tag(unicode_mixed, "system-reminder"),
        "\u{0130}stanbul<\\/system-reminder>"
    );
    // 空内容 / 空 tag 退化安全。
    assert_eq!(escape_closing_tag("", "system-reminder"), "");
    assert_eq!(escape_closing_tag("x</>y", ""), "x<\\/>y");
}

#[test]
fn escape_closing_tag_whitespace_before_gt_variants() {
    // BUG-20261125-007：`>` 前 ASCII 空白 run 变体与精确形态同样构成闭合
    // （HTML 规范：标签名与 `>` 之间的空白合法，token 仍是 end tag）。
    // 空格 / 多空格 / tab / 换行 / 混合，全部命中。
    for variant in [
        "</system-reminder >",
        "</system-reminder   >",
        "</system-reminder\t>",
        "</system-reminder\n>",
        "</system-reminder \t \n >",
        "</SYSTEM-REMINDER >",
    ] {
        let out = escape_closing_tag(&format!("x{variant}y"), "system-reminder");
        assert!(
            out.contains("<\\/") && !out.contains("</system-reminder"),
            "空白变体 {variant:?} 必须被转义：{out:?}"
        );
    }
    // 原始大小写与空白形态保留（只插 `\`，不改写内容）。
    assert_eq!(
        escape_closing_tag("</SYSTEM-REMINDER  >", "system-reminder"),
        "<\\/SYSTEM-REMINDER  >"
    );
    assert_eq!(
        escape_closing_tag("</system-reminder \n>", "system-reminder"),
        "<\\/system-reminder \n>"
    );
    // 属性形态（名字与 `>` 之间夹非空白）不转义——诚实边界 pin。
    assert_eq!(
        escape_closing_tag("</system-reminder foo>", "system-reminder"),
        "</system-reminder foo>"
    );
    // 已逃逸形态不叠加（`\` 使 `</` 头不成立）。
    assert_eq!(
        escape_closing_tag("<\\/system-reminder>", "system-reminder"),
        "<\\/system-reminder>"
    );
    // 近邻形态继续不误伤（`</` 与名字间空白 = bogus comment 非标签）。
    assert_eq!(
        escape_closing_tag("</ system-reminder >", "system-reminder"),
        "</ system-reminder >"
    );
}

// ----------：OWASP CSV Injection 公式前缀守卫 ----------

#[test]
fn csv_formula_prefixes_get_quote_guard() {
    // 五类公式前缀（OWASP CSV Injection / CWE-1236）：Excel/WPS/LibreOffice
    // 打开 CSV 时 = 开头单元格被当公式求值（=HYPERLINK 外呼、=cmd| 形态
    // 命令注入）；+/-/@/\t 是同族变体。守卫 = 前置 `'`（Excel 文本标记，
    // 显示时隐藏；pandas 等非 Excel 消费者读到的 ' 前缀是已知代价）。
    assert_eq!(
        sanitize_csv("=HYPERLINK(\"http://evil\",\"x\")"),
        "\"'=HYPERLINK(\"\"http://evil\"\",\"\"x\"\")\""
    );
    assert_eq!(sanitize_csv("=1+1"), "'=1+1");
    assert_eq!(sanitize_csv("+SUM(A1:A2)"), "'+SUM(A1:A2)");
    assert_eq!(sanitize_csv("-2-2 cmd"), "'-2-2 cmd");
    assert_eq!(sanitize_csv("@SUM(1)"), "'@SUM(1)");
    assert_eq!(sanitize_csv("\t=cmd"), "'\t=cmd");
    // 守卫 + RFC 包裹叠加：公式前缀且含逗号 → ' 内嵌于引号包裹。
    assert_eq!(sanitize_csv("=x,y"), "\"'=x,y\"");
}

#[test]
fn csv_non_formula_fields_unaffected_by_guard() {
    // 无公式前缀的合法字段（含连字符中缀、数字串、空串、中文）零变化。
    assert_eq!(sanitize_csv("task-a1"), "task-a1");
    assert_eq!(sanitize_csv("a-b-c"), "a-b-c");
    assert_eq!(sanitize_csv("42"), "42");
    assert_eq!(sanitize_csv(""), "");
    assert_eq!(sanitize_csv("节点 甲"), "节点 甲");
}

#[test]
fn model_identifier_accepts_valid_wire_forms() {
    use super::validate_model_identifier;
    // 常规两段式；嵌套 `/`（业界 vendor/org/model:variant 形态）合法。
    assert!(validate_model_identifier("openai/gpt-4o").is_ok());
    assert!(validate_model_identifier("test/testai-1.1").is_ok());
    assert!(validate_model_identifier("openrouter/deepseek/deepseek-chat").is_ok());
    assert!(validate_model_identifier("zhipu/glm-4.7").is_ok());
}

#[test]
fn model_identifier_rejects_traversal_and_malformed() {
    use super::validate_model_identifier;
    // 实机实测放行形态：vendor=`..`（含 / 所以过旧 contains('/') 闸）。
    assert!(validate_model_identifier("../../etc/passwd").is_err());
    // `..` 出现在任何段都拒（尾段形态也一样是穿越语义）。
    assert!(validate_model_identifier("openai/../etc").is_err());
    assert!(validate_model_identifier("openai/..").is_err());
    // 单 `.` 段同样拒（空段语义）。
    assert!(validate_model_identifier("openai/.").is_err());
    // 空 / 纯空白语义：空串拒；无 `/` 拒（保持旧 CLI 校验意图）。
    assert!(validate_model_identifier("").is_err());
    assert!(validate_model_identifier("gpt-4o").is_err());
    // 空段（首/中/尾）拒。
    assert!(validate_model_identifier("/gpt-4o").is_err());
    assert!(validate_model_identifier("openai//gpt").is_err());
    assert!(validate_model_identifier("openai/").is_err());
    // 反斜杠（Windows 路径分隔）与控制字符拒。
    assert!(validate_model_identifier(r"openai\gpt-4o").is_err());
    assert!(validate_model_identifier("openai/gpt\n-4o").is_err());
}

#[test]
fn model_entry_name_bare_form_is_legal_but_traversal_still_rejected() {
    use super::validate_model_entry_name;
    // 宽松档：裸名合法（dashboard 既有契约，glm-5.3-flash 先例）。
    assert!(validate_model_entry_name("glm-5.3-flash").is_ok());
    assert!(validate_model_entry_name("gpt-5.6-sol").is_ok());
    // 段级检查对裸名同样生效：裸 `..` / 反斜杠 / 控制字符 / 空串拒。
    assert!(validate_model_entry_name("..").is_err());
    assert!(validate_model_entry_name("../etc").is_err());
    assert!(validate_model_entry_name(r"openai\gpt").is_err());
    assert!(validate_model_entry_name("").is_err());
    assert!(validate_model_entry_name("a/").is_err());
}

#[test]
fn windows_reserved_device_names_get_underscore_prefix() {
    use super::is_windows_reserved_device_base;
    // BUG-20261111-083：裸 `nul` 在 Win11 26200 实测仍设备解析
    // （File::create 报成功但文件永不落盘）；其余 21 名在旧 Windows
    // （10/Server）同样解析。出口统一前缀 `_` 断开设备名语义。
    for name in [
        "con", "CON", "nul", "Nul", "aux", "prn", "com1", "com9", "lpt1", "lpt9",
    ] {
        assert_eq!(
            sanitize_path_segment(name),
            format!("_{name}"),
            "{name} must be neutralized"
        );
        assert!(is_windows_reserved_device_base(name), "{name} predicate");
    }
    // 扩展名形态按基名命中（经典 Win32 语义）。
    assert_eq!(sanitize_path_segment("con.txt"), "_con.txt");
    assert_eq!(sanitize_path_segment("nul.jsonl"), "_nul.jsonl");
    // 尾点先修剪再判基名。
    assert_eq!(sanitize_path_segment("con."), "_con");
    // 非保留词直通不受影响（com0/lpt0/com10 不在名单）。
    for ok in [
        "console",
        "config",
        "com0",
        "lpt0",
        "com10",
        "con_x",
        "conx",
        "auxiliary",
    ] {
        assert_eq!(sanitize_path_segment(ok), ok, "{ok} must pass through");
        assert!(!is_windows_reserved_device_base(ok), "{ok} predicate");
    }
    // 会话键族恒含分隔符映射出的 `_`，永不为裸保留名——既有映射不变量不受影响。
    assert_eq!(
        sanitize_path_segment("agent:main:session:1"),
        "agent_main_session_1"
    );
    // 前缀把原 ≤80 的保留名输入推进 hash 分支时，输出仍是 78 字符 hash 形态
    // （分离不变量：hash 形态只出自 hash 分支）。
    let long = format!("nul.{}", "a".repeat(76)); // 恰 80 字符直通域
    let out = sanitize_path_segment(&long);
    assert_eq!(out.chars().count(), 78);
    assert_eq!(&out[60..62], "__");
}

// ---------------------------------------------------------------------------
// redact_url_secrets（BUG-20261123-025 单一真相源提升：channels base 原版
// 语义逐字移植 + 同款用例；channels 侧薄委托后其测试继续钉委托面）
// ---------------------------------------------------------------------------

#[test]
fn redact_url_secrets_masks_key_query_param() {
    let msg = "webhook https://oapi.dingtalk.com/robot/send?access_token=SECRET123 delivery failed";
    let out = redact_url_secrets(msg);
    assert!(out.contains("access_token=<redacted>"), "{out}");
    assert!(!out.contains("SECRET123"), "{out}");
    assert!(out.contains("delivery failed"), "{out}");
}

#[test]
fn redact_url_secrets_masks_access_token_and_bearer_shapes() {
    let out = redact_url_secrets(
        "u?access_token=AAA&token=BBB&secret=CCC&sign=DDD&ticket=EEE&key=FFF&webhook_key=GGG end",
    );
    assert!(out.contains("access_token=<redacted>"), "{out}");
    assert!(out.contains("token=<redacted>"), "{out}");
    assert!(out.contains("secret=<redacted>"), "{out}");
    assert!(out.contains("sign=<redacted>"), "{out}");
    assert!(out.contains("ticket=<redacted>"), "{out}");
    assert!(out.contains("key=<redacted>"), "{out}");
    assert!(out.contains("webhook_key=<redacted>"), "{out}");
    assert!(!out.contains("AAA"), "{out}");
    assert!(!out.contains("GGG"), "{out}");
}

#[test]
fn redact_url_secrets_boundary_guards_prefix_collisions() {
    // access_token= 中的 token= 子串前面是 `_`，不命中边界 → 不二次改写
    let out = redact_url_secrets("u?access_token=AAA&token=BBB");
    assert!(out.contains("access_token=<redacted>"), "{out}");
    // web_token= / my_key= 这类非起头参数名（前面不是 ?/&）不改写
    let out2 = redact_url_secrets("u?webhook_key=KKK");
    assert!(out2.contains("webhook_key=<redacted>"), "{out2}");
    let out3 = redact_url_secrets("u?my_key=VVV");
    assert!(out3.contains("my_key=VVV"), "{out3}");
}

#[test]
fn redact_url_secrets_value_terminators() {
    // 值截至 & / 空白 / 引号 / 右括号
    assert_eq!(
        redact_url_secrets("u?token=AAA&x=1"),
        "u?token=<redacted>&x=1"
    );
    assert_eq!(
        redact_url_secrets("u?token=AAA x=1"),
        "u?token=<redacted> x=1"
    );
    assert_eq!(redact_url_secrets("u?token=AAA\""), "u?token=<redacted>\"");
    assert_eq!(redact_url_secrets("(u?token=AAA)"), "(u?token=<redacted>)");
    // 串尾无终止符 = 值到结尾
    assert_eq!(redact_url_secrets("u?token=AAATAIL"), "u?token=<redacted>");
    // 非凭据参数原样保留诊断价值
    assert_eq!(redact_url_secrets("u?a=1&b=2"), "u?a=1&b=2");
    // 空串/无 query 原样
    assert_eq!(
        redact_url_secrets("https://example.com/hook"),
        "https://example.com/hook"
    );
}

#[test]
fn sanitize_agent_id_confines_and_keeps_uuid_identity() {
    use super::sanitize_agent_id;
    // 穿越形态逐字符替换 `_`：无 `..`、无分隔符、无盘符冒号。
    for hostile in [r"..\..\evil", "../../evil", "a/b", "a:b", "a\nb"] {
        let out = sanitize_agent_id(hostile);
        assert!(
            !out.contains('\\') && !out.contains('/') && !out.contains(':') && !out.contains('\n'),
            "hostile={hostile:?} 消毒后不得含分隔/穿越/控制字符: {out:?}"
        );
        // 输出只含白名单字符（字母数字/_/-）——confine 语义而非全替换
        // （`evil` 等字母数字段合法直通）。
        assert!(
            out.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "hostile={hostile:?} 消毒后不得含白名单外字符: {out:?}"
        );
        assert!(!out.contains(".."), "hostile={hostile:?} 不得残留穿越组件");
    }
    // 生产形态（uuid simple hex）恒等——消毒零破坏。
    assert_eq!(
        sanitize_agent_id("0af9c1e2d3b4"),
        "0af9c1e2d3b4",
        "uuid 形态必须恒等"
    );
    // 常规带分隔符 id（agent_x-1）恒等。
    assert_eq!(sanitize_agent_id("agent_x-1"), "agent_x-1");
}
