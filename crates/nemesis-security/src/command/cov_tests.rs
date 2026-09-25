// command.rs 覆盖率补充测试（detect_self_recursive_function 的命中收尾
// 319——CMD-10 自递归命名函数 fork bomb 判定）。

use super::*;

/// `bomb(){ bomb|bomb & };bomb` 形态：体内出现自身名 → 判定命中（319）。
#[test]
fn check_blocks_self_recursive_function_fork_bomb() {
    let guard = Guard::new(true);

    let err = guard.check("bomb(){ bomb | bomb & };bomb").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("fork bomb") && msg.contains("bomb"),
        "必须报自递归函数 fork bomb: {msg}"
    );
}

/// 对照：同名前缀但非函数定义（无 `(){`）不触发该检测。
#[test]
fn non_function_definition_does_not_trigger_recursion_check() {
    let guard = Guard::new(true);
    // 普通调用（未命中任何封锁模式时为 Ok；此处只关心不是 fork-bomb 错误）。
    match guard.check("bomb --version") {
        Ok(()) => {}
        Err(e) => assert!(
            !e.to_string().contains("fork bomb"),
            "普通调用不得误判 fork bomb: {e}"
        ),
    }
}
