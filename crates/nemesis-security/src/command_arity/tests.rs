//! command_arity 单测（B5）：分词 + 归约表 + env 前缀跳过，30+ 常见命令。

use super::*;

// ---------- tokenize_command ----------

#[test]
fn tokenize_plain() {
    assert_eq!(
        tokenize_command("cargo build --release"),
        vec!["cargo", "build", "--release"]
    );
}

#[test]
fn tokenize_collapses_whitespace() {
    assert_eq!(
        tokenize_command("cargo   build\t --release"),
        vec!["cargo", "build", "--release"]
    );
}

#[test]
fn tokenize_double_quotes_keep_spaces() {
    assert_eq!(
        tokenize_command(r#"node "my script.js" --port 3000"#),
        vec!["node", "my script.js", "--port", "3000"]
    );
}

#[test]
fn tokenize_single_quotes_keep_spaces() {
    assert_eq!(
        tokenize_command("git commit -m 'fix two bugs'"),
        vec!["git", "commit", "-m", "fix two bugs"]
    );
}

#[test]
fn tokenize_windows_backslash_literal() {
    // 反斜杠不转义：Windows 路径照字面保留
    assert_eq!(
        tokenize_command(r#"type "C:\Program Files\app\config.json""#),
        vec!["type", r"C:\Program Files\app\config.json"]
    );
}

#[test]
fn tokenize_empty_and_whitespace() {
    assert!(tokenize_command("").is_empty());
    assert!(tokenize_command("   \t  ").is_empty());
}

#[test]
fn tokenize_unclosed_quote_keeps_tail() {
    // 未闭合引号：内容保留为单 token（诚实退化，不丢内容）
    assert_eq!(tokenize_command("echo \"hello"), vec!["echo", "hello"]);
}

// ---------- reduce_command：表命中 ----------

#[test]
fn reduce_git_short_is_exact() {
    // ≤ arity：原样输出，不加 *
    assert_eq!(reduce_command("git status"), "git status");
}

#[test]
fn reduce_git_commit() {
    assert_eq!(reduce_command("git commit -m \"fix bug\""), "git commit *");
}

#[test]
fn reduce_git_flag_before_subcommand() {
    // 退化形态可接受：`git -C *`（arity 语义如实截断）
    assert_eq!(reduce_command("git -C ../sub log --oneline -5"), "git -C *");
}

#[test]
fn reduce_git_bare() {
    assert_eq!(reduce_command("git"), "git");
}

#[test]
fn reduce_cargo_build() {
    assert_eq!(
        reduce_command("cargo build --release --features foo"),
        "cargo build *"
    );
}

#[test]
fn reduce_cargo_test() {
    assert_eq!(
        reduce_command("cargo test -p nemesis-agent --lib"),
        "cargo test *"
    );
}

#[test]
fn reduce_cargo_bare() {
    assert_eq!(reduce_command("cargo"), "cargo");
}

#[test]
fn reduce_npm_install() {
    assert_eq!(reduce_command("npm install -g pnpm"), "npm install *");
}

#[test]
fn reduce_npm_run_longest_match_wins() {
    // "npm run"(3) 优先于 "npm"(2)：保留到子命令
    assert_eq!(
        reduce_command("npm run dev -- --port=3000"),
        "npm run dev *"
    );
}

#[test]
fn reduce_npm_run_exact() {
    assert_eq!(reduce_command("npm run"), "npm run");
}

#[test]
fn reduce_npx() {
    assert_eq!(
        reduce_command("npx create-vite my-app --template vue-ts"),
        "npx create-vite *"
    );
}

#[test]
fn reduce_docker_run() {
    assert_eq!(reduce_command("docker run -it ubuntu bash"), "docker run *");
}

#[test]
fn reduce_docker_compose_multi_token_key() {
    assert_eq!(
        reduce_command("docker compose up -d --build"),
        "docker compose up *"
    );
}

#[test]
fn reduce_docker_compose_hyphenated() {
    assert_eq!(
        reduce_command("docker-compose build --no-cache"),
        "docker-compose build *"
    );
}

#[test]
fn reduce_kubectl_get() {
    assert_eq!(reduce_command("kubectl get pods -n prod"), "kubectl get *");
}

#[test]
fn reduce_kubectl_apply() {
    assert_eq!(
        reduce_command("kubectl apply -f manifest.yaml"),
        "kubectl apply *"
    );
}

#[test]
fn reduce_pip_install() {
    assert_eq!(
        reduce_command("pip install requests==2.31.0"),
        "pip install *"
    );
}

#[test]
fn reduce_python_m() {
    assert_eq!(reduce_command("python -m pytest -q tests/"), "python -m *");
}

#[test]
fn reduce_python3_script() {
    assert_eq!(
        reduce_command("python3 script.py --flag"),
        "python3 script.py *"
    );
}

#[test]
fn reduce_node_script() {
    assert_eq!(
        reduce_command("node server.js --port 3000"),
        "node server.js *"
    );
}

#[test]
fn reduce_node_quoted_script_with_space() {
    // 引号内空格：分词保住完整文件名，arity 2 → 前两个 token
    assert_eq!(
        reduce_command("node \"my script.js\" --watch"),
        "node my script.js *"
    );
}

#[test]
fn reduce_go_test() {
    assert_eq!(reduce_command("go test ./..."), "go test *");
}

#[test]
fn reduce_dotnet_build() {
    assert_eq!(
        reduce_command("dotnet build --configuration Release"),
        "dotnet build *"
    );
}

#[test]
fn reduce_winget_install() {
    assert_eq!(
        reduce_command("winget install Git.Git --silent"),
        "winget install *"
    );
}

#[test]
fn reduce_systemctl_restart() {
    assert_eq!(
        reduce_command("systemctl restart nginx"),
        "systemctl restart *"
    );
}

#[test]
fn reduce_gh_pr() {
    assert_eq!(reduce_command("gh pr create --title x"), "gh pr *");
}

// ---------- reduce_command：env 前缀跳过 ----------

#[test]
fn reduce_env_prefix_skipped() {
    assert_eq!(
        reduce_command("FOO=1 cargo build --release"),
        "cargo build *"
    );
}

#[test]
fn reduce_env_prefix_multiple() {
    // 剩余 token 恰好 = keep：原样输出（无 star）
    assert_eq!(reduce_command("A=1 B=2 npm run dev"), "npm run dev");
}

#[test]
fn reduce_env_prefix_empty_value() {
    assert_eq!(reduce_command("RUST_LOG= cargo check"), "cargo check");
}

#[test]
fn reduce_env_only_command_is_empty() {
    // 只有 env 赋值：无命令头，诚实返回空
    assert_eq!(reduce_command("FOO=1"), "");
}

#[test]
fn reduce_flag_value_not_env() {
    // `--flag=1` 不是 env 前缀（首字符 `-`）：作为普通 token 保留
    assert_eq!(
        reduce_command("unknown-tool --flag=1 value"),
        "unknown-tool *"
    );
}

// ---------- reduce_command：表未命中退首 token ----------

#[test]
fn reduce_unknown_head_falls_back_to_first_token() {
    assert_eq!(
        reduce_command("unknown-tool --flag value"),
        "unknown-tool *"
    );
}

#[test]
fn reduce_rm_falls_back() {
    assert_eq!(reduce_command("rm -rf /tmp/build"), "rm *");
}

#[test]
fn reduce_ls_single_token_no_star() {
    assert_eq!(reduce_command("ls"), "ls");
}

// ---------- reduce_command：边界 ----------

#[test]
fn reduce_empty_string() {
    assert_eq!(reduce_command(""), "");
}

#[test]
fn reduce_whitespace_only() {
    assert_eq!(reduce_command("   \t "), "");
}

#[test]
fn reduce_extra_whitespace() {
    assert_eq!(reduce_command("cargo   build   --release"), "cargo build *");
}

#[test]
fn reduce_shell_wrapper_degrades_to_first_token() {
    // 已知边界：shell 包装不展开，退首 token
    assert_eq!(reduce_command("cmd /c git status"), "cmd *");
}

#[test]
fn reduce_longest_match_beats_shorter_high_arity_first() {
    // 表序无关性：无论 "pnpm"(2) 与 "pnpm run"(3) 谁先，最长匹配胜出
    assert_eq!(reduce_command("pnpm run build --prod"), "pnpm run build *");
}

// ---------- CMD-12（2026-10-13）：command_segments 顶层复合分段 ----------

use crate::command_arity::command_segments;

fn segs(cmd: &str) -> Vec<String> {
    command_segments(cmd)
}

#[test]
fn test_segments_split_top_level_operators() {
    assert_eq!(segs("cd /tmp && rm -rf y"), vec!["cd /tmp", "rm -rf y"]);
    assert_eq!(segs("echo hi||rm x"), vec!["echo hi", "rm x"]);
    assert_eq!(segs("a; b ;c"), vec!["a", "b", "c"]);
    assert_eq!(
        segs("cargo build | tee out.txt"),
        vec!["cargo build", "tee out.txt"]
    );
    // 单 `&`（后台）同族切分
    assert_eq!(segs("make & make install"), vec!["make", "make install"]);
    // 换行同族
    assert_eq!(segs("a\nb"), vec!["a", "b"]);
}

#[test]
fn test_segments_quoted_operators_not_split() {
    // 引号内的操作符是字面量，不切
    assert_eq!(segs("echo \"a && b\""), vec!["echo \"a && b\""]);
    assert_eq!(segs("grep 'x|y' file"), vec!["grep 'x|y' file"]);
    // 单引号内的双引号不开启双引号态
    assert_eq!(segs("echo '\" && \"'"), vec!["echo '\" && \"'"]);
}

#[test]
fn test_segments_unbalanced_quote_blind_splits() {
    // 引号不闭合 → quote-blind 盲切（复检批改向，多角度审计批固化）：
    // 旧 fallback「退单段」是 fail-open——deny 整串锚定打不中非首段，
    // 一个撇号就能整体击穿。盲切方向论证见 command_segments 文档：
    // cmd.exe（本项目 Windows 主执行路径）视 `'` 为普通字符，盲切对齐
    // 真实解析器；POSIX 下残引号本就是残命令。
    assert_eq!(
        segs("echo \"hi && rm -rf y"),
        vec!["echo \"hi", "rm -rf y"],
        "盲切把操作符真实切开，deny 任一段命中即拦"
    );
    // 英文撇号叙述：cmd.exe 语义下 `'` 是字面量，`&&` 真切分
    assert_eq!(
        segs("echo it's done && git push"),
        vec!["echo it's done", "git push"]
    );
    // 盲切多段不丢段（`a ' && b '` 是**偶数**引号 → quote-aware 域，
    // 引号真引住了操作符，不切——那条语义由
    // test_segments_quoted_operators_not_split 钉）
    assert_eq!(
        segs("echo 'a && rm x && git push"),
        vec!["echo 'a", "rm x", "git push"],
        "奇数引号盲切域内多段全保留"
    );
}

#[test]
fn test_segments_redirection_amp_not_split() {
    // `2>&1` 的 `&` 前导是 `>`，是重定向不是操作符
    assert_eq!(
        segs("cargo build > log.txt 2>&1"),
        vec!["cargo build > log.txt 2>&1"]
    );
    assert_eq!(segs("cargo test 2>&1"), vec!["cargo test 2>&1"]);
    // `>&` 出现在段中也不切
    assert_eq!(
        segs("app --log 2>&1 && echo done"),
        vec!["app --log 2>&1", "echo done"]
    );
    // `&>`（bash 合并重定向）：`&` 后继是 `>`，同族不切
    assert_eq!(
        segs("cargo build &> log.txt"),
        vec!["cargo build &> log.txt"]
    );
    // `>|`（强制覆盖重定向，bash）：`|` 前导 `>` 不切
    assert_eq!(segs("cmd >| out.txt"), vec!["cmd >| out.txt"]);
    // `<&N`（fd 复制重定向，bash）：`&` 前导 `<` 不切（闭环批 2026-10-13
    // 补齐——此前 `cmd <&0` 把 `&` 当操作符切出假段 `&0`）
    assert_eq!(segs("cmd <&0"), vec!["cmd <&0"]);
    assert_eq!(
        segs("cmd <&0 && echo done"),
        vec!["cmd <&0", "echo done"],
        "<&N 随段保留后，后续真操作符照常切"
    );
    // `b &` 的 `&` 前导不是 `<`/`>`：`a<b & c` 的 `&` 是真后台操作符
    assert_eq!(segs("a<b & c"), vec!["a<b", "c"]);
}

#[test]
fn test_segments_empty_and_single_segment_pins() {
    // 连续操作符产生空段：跳过
    assert_eq!(segs("a && && b"), vec!["a", "b"]);
    assert_eq!(segs("a;;b"), vec!["a", "b"]);
    // 纯操作符 → 空
    assert!(command_segments("&&").is_empty());
    assert!(command_segments("").is_empty());
    assert!(command_segments("   ").is_empty());
    // 无操作符 = 单段（旧整串语义锚点）
    assert_eq!(segs("cargo build --release"), vec!["cargo build --release"]);
    assert_eq!(segs("git status"), vec!["git status"]);
}

#[test]
fn test_segments_trims_and_dedups() {
    assert_eq!(segs("  a  &&  b  "), vec!["a", "b"]);
    // 相邻重复段塌缩（any/all 语义不变）
    assert_eq!(segs("cargo test && cargo test"), vec!["cargo test"]);
}

// ---------- strip_env_assignments（分段后 env 剥离，deny 口径与 reduce 表分流一致） ----------

use crate::command_arity::strip_env_assignments;

#[test]
fn test_strip_env_prefix_before_operator() {
    // env 前缀只属于 `&&` 左段；不剥会把 `cargo *` 的 deny 打空
    assert_eq!(strip_env_assignments("FOO=1 cargo build"), "cargo build");
    assert_eq!(strip_env_assignments("A=1 B= git push"), "git push");
    // 全 env 无命令 → 空串（allow 侧空段守卫 + deny 锚定联合语义照常）
    assert_eq!(strip_env_assignments("FOO=1"), "");
    // 非 env 首 token 原样保留
    assert_eq!(strip_env_assignments("rm -rf x"), "rm -rf x");
    // 契约边界（文档：输入须是 trim 过的段）：空白段无 token 可剥，原样
    // 返回——调用端（auditor/CLI 预览）输入恒为 trim 后的段，不在此猜。
    assert_eq!(strip_env_assignments("   "), "   ");
}
