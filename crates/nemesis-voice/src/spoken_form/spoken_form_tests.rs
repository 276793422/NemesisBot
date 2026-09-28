//! spoken_form.rs 语料矩阵测试（W7-9；跨平台，常规 CI）。
//! 逐形态钉死计划 §四 W4 表格的清洗行为与 §七-6 固定话术文案。

use super::{
    FUSE_CODE_DENSITY, MAX_PIECES, SPOKEN_CODE, SPOKEN_CODE_INLINE, SPOKEN_FILE, SPOKEN_LINK,
    SPOKEN_MORE, SPOKEN_TABLE, clean_sentence, spoken_pieces,
};

// ---------------------------------------------------------------------------
// 结构级：代码密度熔断 / 围栏代码 / 表格
// ---------------------------------------------------------------------------

#[test]
fn code_density_fuse_over_half() {
    let reply = "说明一下：\n```rust\nfn main() {\n    let a = 1;\n    let b = 2;\n    println!(\"{}\", a + b);\n}\n```\n好";
    let out = spoken_pieces(reply);
    assert_eq!(out, vec![FUSE_CODE_DENSITY.to_string()]);
}

#[test]
fn code_density_below_half_keeps_prose() {
    let reply = "这段解释比较长，解释了背景和原因，还给出了后续建议，因此正文占比明显更高。\n```python\nprint(1)\n```\n以上就是全部内容。";
    let out = spoken_pieces(reply);
    assert!(!out.contains(&FUSE_CODE_DENSITY.to_string()));
    assert!(out.iter().any(|p| p.contains(SPOKEN_CODE)));
    assert!(out.iter().any(|p| p.contains("解释")));
}

#[test]
fn fenced_code_block_becomes_placeholder() {
    let out = spoken_pieces("看这段：\n```\ncode here\n```");
    assert!(out.contains(&SPOKEN_CODE.to_string()), "{out:?}");
    assert!(!out.iter().any(|p| p.contains("code here")));
}

#[test]
fn unterminated_fence_treated_as_code() {
    // prose 占比足够大（不触发密度熔断），围栏未闭合 → 剩余全部按代码丢弃
    let reply = "这段示例演示了如何初始化客户端并建立连接，请看下面的代码片段，注意超时参数的设置方式。示例：\n```js\nconsole.log(1);";
    let out = spoken_pieces(reply);
    assert!(out.contains(&SPOKEN_CODE.to_string()), "{out:?}");
    assert!(!out.iter().any(|p| p.contains("console")));
}

#[test]
fn table_reports_row_count() {
    let out = spoken_pieces("结果如下：\n| 名字 | 数量 |\n|---|---|\n| 甲 | 1 |\n| 乙 | 2 |");
    let expected = format!("{} 4 行", SPOKEN_TABLE);
    assert!(out.contains(&expected), "{out:?}");
    assert!(!out.iter().any(|p| p.contains("甲")));
}

// ---------------------------------------------------------------------------
// 行内级：链接 / 路径 / 行内代码 / 强调 / 行首形态 / 符号
// ---------------------------------------------------------------------------

#[test]
fn markdown_link_keeps_label() {
    let out = clean_sentence("参考[官方文档](https://example.com/a)即可");
    assert!(out.contains("官方文档"), "{out}");
    assert!(out.contains(SPOKEN_LINK), "{out}");
    assert!(!out.contains("https"), "{out}");
}

#[test]
fn bare_url_replaced() {
    let out = clean_sentence("详情见 https://example.com/x?y=1 说明");
    assert!(out.contains(SPOKEN_LINK), "{out}");
    assert!(!out.contains("example.com"), "{out}");
}

#[test]
fn windows_path_replaced() {
    let out = clean_sentence("配置在 C:\\Users\\me\\config.json 里");
    assert!(out.contains(SPOKEN_FILE), "{out}");
    assert!(!out.contains("config.json"), "{out}");
}

#[test]
fn unix_path_replaced() {
    let out = clean_sentence("日志在 /var/log/app/main.log 目录");
    assert!(out.contains(SPOKEN_FILE), "{out}");
    assert!(!out.contains("main.log"), "{out}");
}

#[test]
fn relative_path_replaced() {
    let out = clean_sentence("看 ./src/main.rs 这个文件");
    assert!(out.contains(SPOKEN_FILE), "{out}");
}

#[test]
fn slash_between_cjk_is_not_a_path() {
    let out = clean_sentence("这个功能支持开启和/或关闭");
    assert!(!out.contains(SPOKEN_FILE), "{out}");
    assert!(out.contains("和/或"), "{out}");
}

#[test]
fn inline_code_identifier_kept() {
    let out = clean_sentence("运行 `npm_run_build` 即可");
    assert!(out.contains("npm_run_build"), "{out}");
}

#[test]
fn inline_code_complex_replaced() {
    let out = clean_sentence("调用 `foo::bar::<T>()` 完成");
    assert!(out.contains(SPOKEN_CODE_INLINE), "{out}");
    assert!(!out.contains("::"), "{out}");
}

#[test]
fn emphasis_markers_stripped() {
    assert_eq!(clean_sentence("**重要**的*细节*内容"), "重要的细节内容");
}

#[test]
fn heading_and_list_markers_stripped() {
    assert_eq!(clean_sentence("## 标题行"), "标题行");
    assert_eq!(clean_sentence("- 列表项一"), "列表项一");
    assert_eq!(clean_sentence("1. 第一步"), "第一步");
}

#[test]
fn emoji_stripped() {
    let out = clean_sentence("完成了🎉太好了👍");
    assert_eq!(out, "完成了 太好了");
}

#[test]
fn hr_line_dropped() {
    assert_eq!(clean_sentence("---"), "");
}

// ---------------------------------------------------------------------------
// 整体：长度熔断 / 空输入 / 真实语料
// ---------------------------------------------------------------------------

#[test]
fn length_fuse_at_max_pieces() {
    let reply = (1..=10)
        .map(|i| format!("这是第{}句话，用来凑够十个句子。", i))
        .collect::<String>();
    let out = spoken_pieces(&reply);
    assert_eq!(out.len(), MAX_PIECES + 1);
    assert_eq!(out.last().unwrap(), SPOKEN_MORE);
}

#[test]
fn empty_input_gives_empty_output() {
    assert!(spoken_pieces("").is_empty());
    assert!(spoken_pieces("   \n\n  ").is_empty());
}

#[test]
fn pure_prose_passthrough() {
    let out = spoken_pieces("今天天气不错。我们去公园吧！");
    assert_eq!(out, vec!["今天天气不错。", "我们去公园吧！"]);
}

#[test]
fn typical_markdown_reply_corpus() {
    // 真实 agent 回复语料：标题 + 列表 + 行内代码 + 链接 + 围栏代码混合
    let reply = "\
## 部署步骤

1. 先运行 `cargo build --release`
2. 参考[构建文档](https://docs.example.com/build)确认依赖
3. 执行启动脚本

```bash
./scripts/start.sh
```

完成后访问 http://127.0.0.1:49000 即可。祝顺利！";
    let out = spoken_pieces(reply);
    // 不含任何原始 markdown 痕迹
    let joined = out.join(" ");
    assert!(!joined.contains("```"), "{joined}");
    assert!(!joined.contains("**"), "{joined}");
    assert!(!joined.contains("https://"), "{joined}");
    assert!(!joined.contains("http://"), "{joined}");
    // 关键要素在
    assert!(joined.contains("部署步骤"), "{joined}");
    assert!(joined.contains(SPOKEN_CODE_INLINE), "{joined}"); // `cargo build --release` 带空格 → 话术替换
    assert!(!joined.contains("release"), "{joined}");
    assert!(joined.contains(SPOKEN_CODE), "{joined}");
    assert!(joined.contains("构建文档"), "{joined}");
}

#[test]
fn inline_code_with_space_not_identifier() {
    // 带空格的命令串不是简单标识符 → 替换话术（不逐字念参数）
    let out = clean_sentence("先跑 `cargo build --release` 再说");
    assert!(out.contains(SPOKEN_CODE_INLINE), "{out}");
    assert!(!out.contains("release"), "{out}");
}
