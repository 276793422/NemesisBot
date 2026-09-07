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
