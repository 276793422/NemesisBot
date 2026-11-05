//! 注释治理 AST 等价闸：
//! 对相对基线修订有改动的每个 .rs 文件，取「基线版本」与「工作区版本」各用 syn 解析，
//! 把文档注释（///、//!、#[doc = "..."]）的载荷归一化为占位符后做规范化序列化哈希比对。
//! 哈希全等 = 语法树逐节点相同 = 证明除注释文本外没有任何代码变化。
//!
//! - 非文档注释（// 与 /* */）不入语法树，其变化天然不可见（也不可能影响编译）；
//! - 字符串字面量、cfg 属性、宏体都在树内，被误改即闸红；
//! - 新增文件无基线版本，自动跳过（不在注释手术证明范围）；
//! - 删除文件如实报告（注释手术不应删除文件）。
//!
//! 用法：comment-gate --baseline <rev> [--repo <path>]

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::Command;

/// 把所有 doc 注释载荷替换为占位符，使注释文本差异不影响语法树比较。
fn normalize_docs(file: syn::File) -> syn::File {
    struct Norm;
    impl syn::fold::Fold for Norm {
        fn fold_attribute(&mut self, attr: syn::Attribute) -> syn::Attribute {
            let attr = syn::fold::fold_attribute(self, attr);
            if attr.path().is_ident("doc") {
                if let syn::Meta::NameValue(nv) = &attr.meta {
                    if matches!(nv.value, syn::Expr::Lit(ref l) if matches!(l.lit, syn::Lit::Str(_)))
                    {
                        let mut out = attr.clone();
                        if let syn::Meta::NameValue(nv2) = &mut out.meta {
                            nv2.value = syn::Expr::Lit(syn::ExprLit {
                                attrs: Vec::new(),
                                lit: syn::Lit::Str(syn::LitStr::new(
                                    "_",
                                    proc_macro2::Span::call_site(),
                                )),
                            });
                        }
                        return out;
                    }
                }
            }
            attr
        }
    }
    syn::fold::fold_file(&mut Norm, file)
}

fn ast_digest(src: &str) -> Result<String> {
    let file: syn::File = syn::parse_str(src).map_err(|e| anyhow::anyhow!("parse error: {e}"))?;
    let norm = normalize_docs(file);
    let mut h = Sha256::new();
    h.update(format!("{norm:#?}").as_bytes());
    Ok(format!("{:x}", h.finalize()))
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .context("git spawn failed")?;
    if !out.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn main() -> Result<()> {
    let mut baseline: Option<String> = None;
    let mut repo = std::env::current_dir()?;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--baseline" => baseline = Some(args.next().expect("--baseline needs value")),
            "--repo" => repo = args.next().expect("--repo needs value").into(),
            other => bail!("unknown arg: {other}"),
        }
    }
    let baseline = baseline.expect("usage: comment-gate --baseline <rev> [--repo <path>]");

    // 枚举相对基线变更的 .rs 文件：M=比较，A=跳过（新文件），D=如实报告
    let mut red = 0usize;
    let mut checked = 0usize;
    for status_line in git(&repo, &["diff", "--name-status", &baseline, "--", "*.rs"])?
        .lines()
        .filter(|l| !l.trim().is_empty())
    {
        let (status, path) = status_line
            .split_once('\t')
            .map(|(s, p)| (s.trim().to_string(), p.to_string()))
            .with_context(|| format!("bad name-status line: {status_line}"))?;
        match status.as_str() {
            "A" | "??" => {
                println!("SKIP   (new file, no baseline): {path}");
                continue;
            }
            "D" => {
                println!("RED    (deleted file): {path}");
                red += 1;
                continue;
            }
            _ => {}
        }
        let base_src = git(&repo, &["show", &format!("{baseline}:{path}")])
            .with_context(|| format!("baseline version unavailable: {path}"))?;
        let head_src = std::fs::read_to_string(repo.join(&path))
            .with_context(|| format!("worktree version unreadable: {path}"))?;
        checked += 1;
        match (ast_digest(&base_src), ast_digest(&head_src)) {
            (Ok(b), Ok(h)) if b == h => println!("GREEN  {path}"),
            (Ok(b), Ok(h)) => {
                red += 1;
                println!("RED    {path} (ast differs: base={b} head={h})");
                report_first_diff_item(&base_src, &head_src, &path);
                // 归一化 Debug 全文落盘（首个 RED 文件），供人工比对定位盲区
                if !path.contains("comment-gate")
                    && let (Ok(base_file), Ok(head_file)) = (
                        syn::parse_str::<syn::File>(&base_src),
                        syn::parse_str::<syn::File>(&head_src),
                    )
                {
                    let dump = format!("{:#?}", normalize_docs(base_file));
                    let _ = std::fs::write("comment-gate-base-dump.txt", dump);
                    let dump = format!("{:#?}", normalize_docs(head_file));
                    let _ = std::fs::write("comment-gate-head-dump.txt", dump);
                }
            }
            (e, h) => {
                red += 1;
                let hb: String = h
                    .as_ref()
                    .map(|s| s.chars().take(8).collect())
                    .unwrap_or_default();
                let side = if e.is_err() { "baseline" } else { "worktree" };
                let err = e.as_ref().err().or_else(|| h.as_ref().err()).unwrap();
                println!("RED    {path} (parse error on {side}, head_digest={hb}): {err}");
            }
        }
    }
    println!("----\nchecked {checked} modified .rs files, {red} red");
    if red > 0 {
        bail!("{red} file(s) failed the AST equivalence gate");
    }
    Ok(())
}

/// 定位第一个不同的顶层 item，便于人眼定位（索引 + 种类 + 预览行）。
fn report_first_diff_item(base: &str, head: &str, path: &str) {
    let pb = syn::parse_str::<syn::File>(base).ok();
    let ph = syn::parse_str::<syn::File>(head).ok();
    if let (Some(b), Some(h)) = (pb, ph) {
        for (i, (ib, ih)) in b.items.iter().zip(h.items.iter()).enumerate() {
            if format!("{ib:#?}") != format!("{ih:#?}") {
                let kind = |it: &syn::Item| match it {
                    syn::Item::Const(_) => "const",
                    syn::Item::Enum(_) => "enum",
                    syn::Item::Fn(_) => "fn",
                    syn::Item::Impl(_) => "impl",
                    syn::Item::Macro(_) => "macro",
                    syn::Item::Mod(_) => "mod",
                    syn::Item::Static(_) => "static",
                    syn::Item::Struct(_) => "struct",
                    syn::Item::Trait(_) => "trait",
                    syn::Item::Type(_) => "type",
                    syn::Item::Use(_) => "use",
                    _ => "other",
                };
                println!(
                    "       first differing item: index {i}, kind={} vs {}",
                    kind(ib),
                    kind(ih)
                );
                return;
            }
        }
        if b.items.len() != h.items.len() {
            println!(
                "       item count differs: base={} head={}",
                b.items.len(),
                h.items.len()
            );
        }
    }
    let _ = path;
}
