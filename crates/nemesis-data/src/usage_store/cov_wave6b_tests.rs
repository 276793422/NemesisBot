//! usage_store.rs 覆盖率收尾（Wave6B）：`get_request_log` 的行解码错误臂
//! （334 行：`Some(Err(e))` → 带上下文的 Err）。
//!
//! 手法：SQLite 动态类型——绕过类型化 insert API，直接往 status_code
//! （INTEGER affinity）塞非数值 TEXT，row.get::<_, i64> 解码即失败。
//! tempfile 不在本 crate dev-deps，沿本目录惯例用 `std::env::temp_dir`。

use super::*;
use std::path::PathBuf;

fn temp_db(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "nemesis_data_usage_store_covw6b_{}_{}.db",
        std::process::id(),
        tag
    ))
}

/// status_code 存了非数值 TEXT → row_to_log 解码失败 → Err（不是 Ok(None)）。
#[test]
fn get_request_log_reports_row_decode_error() {
    let db = temp_db("decode_err");
    let _ = std::fs::remove_file(&db);
    let store = DataStore::open(&db).expect("open");

    // 原始 SQL 注入坏行（INTEGER 列存非数值 TEXT——schema 层不拦）。
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO request_logs (trace_id, model, created_at, status_code)
             VALUES ('covw6b', 'm', 1, 'not-a-number')",
            [],
        )
        .expect("坏行必须能写入（动态类型）");
    }

    let err = store.get_request_log(1).expect_err("解码失败必须走 Err 臂");
    assert!(
        err.contains("get_request_log row"),
        "必须带行级 context: {err}"
    );

    let _ = std::fs::remove_file(&db);
}
