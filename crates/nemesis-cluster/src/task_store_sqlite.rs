//! `SqliteTaskStore` → `TaskStore` trait 适配桥。
//!
//! trait 住在 nemesis-cluster（所有方），SQLite 实现体住在
//! nemesis-taskstore——orphan rule 决定 impl 必须落本 crate。
//! 语义对齐由 taskstore 侧保证（错误文案/盖章规则/条件更新原子性
//! 逐条镜像 InMemory）；此处只做签名搬运，外加 list 面的 Err→空
//! 列表降级（trait 返回 Vec 不带 Result；mid-run 磁盘故障响亮
//! error 留痕，open 期损坏已被 fail-closed 拦在装配之前）。

use nemesis_taskstore::SqliteTaskStore;

use crate::task_manager::TaskStore;

use nemesis_types::cluster::{Task, TaskStatus};

impl TaskStore for SqliteTaskStore {
    fn create(&self, task: Task) -> Result<(), String> {
        SqliteTaskStore::create(self, task)
    }

    fn get(&self, task_id: &str) -> Result<Task, String> {
        SqliteTaskStore::get(self, task_id)
    }

    fn update_result(
        &self,
        task_id: &str,
        status: TaskStatus,
        result: Option<serde_json::Value>,
    ) -> Result<(), String> {
        SqliteTaskStore::update_result(self, task_id, status, result)
    }

    fn update_result_if_status(
        &self,
        task_id: &str,
        expected: &[TaskStatus],
        status: TaskStatus,
        result: Option<serde_json::Value>,
    ) -> Result<bool, String> {
        SqliteTaskStore::update_result_if_status(self, task_id, expected, status, result)
    }

    fn delete(&self, task_id: &str) -> Result<(), String> {
        SqliteTaskStore::delete(self, task_id)
    }

    fn list_by_status(&self, status: TaskStatus) -> Vec<Task> {
        SqliteTaskStore::list_by_status(self, status).unwrap_or_else(|e| {
            tracing::error!("[Cluster] tasks.db list_by_status failed, returning empty: {e}");
            Vec::new()
        })
    }

    fn list_all(&self) -> Vec<Task> {
        SqliteTaskStore::list_all(self).unwrap_or_else(|e| {
            tracing::error!("[Cluster] tasks.db list_all failed, returning empty: {e}");
            Vec::new()
        })
    }
}
