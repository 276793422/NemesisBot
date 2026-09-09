//! `BoardStore` — 看板 SQLite 存储（manager 单写者权威）。
//!
//! 线程安全：内部 `Mutex<Connection>`（镜像 `nemesis-data::DataStore`）。
//! 每个写操作维护审计痕迹：状态转移写 `status_change` 评论 + activity_log，
//! 指派/更新/评论写 activity_log 并自动维护订阅者（创建者/被指派者/评论者）。
//! issue 编号在事务内自增（`board_meta.issue_counter`），前缀存
//! `board_meta.number_prefix`（默认 `NB`）。

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};

use crate::assignment::{Actor, AssignmentType};
use crate::db;
use crate::models::{
    ActivityLog, Attachment, Autopilot, AutopilotPatch, BoardAsset, Channel, ChannelMember,
    ChannelMessage, Comment, CommentType, DispatchRecord, Issue, IssueFilter, IssuePatch,
    IssueStatus, LedgerEntry, NewAsset, NewAutopilot, NewChannel, NewChannelMessage, NewComment,
    NewIssue, NewNotification, NewTeamMemory, Notification, PostedMessage, Project, ProjectPatch,
    Subscriber, TeamMemoryEntry, channel_message_type, dispatch_state, notification_kind,
    thread_kind,
};

/// Thread-safe SQLite board store.
pub struct BoardStore {
    conn: Mutex<Connection>,
    /// Swarm M3（§5.4）：dispatch 签发上下文槽（gateway 装配时 set 一次；
    /// 所有 dispatch 函数已持有 store，资产段渲染零参数蔓延）。None =
    /// 本节点未配资产签发——派发 prompt 不带「## 任务资产」段。
    asset_signing: std::sync::OnceLock<crate::asset_token::AssetSignContext>,
}

impl BoardStore {
    /// Open (or create) the board database at `db_path`.
    ///
    /// `prefix` 是 issue 编号前缀（如 `NB` → `NB-1`）；仅首次建库时生效
    /// （之后以 `board_meta.number_prefix` 为准，改前缀需显式迁移）。
    pub fn open(db_path: &Path, prefix: &str) -> Result<Self, String> {
        let conn = db::init_db(db_path)?;
        conn.execute(
            "INSERT OR IGNORE INTO board_meta(key, value) VALUES('number_prefix', ?1)",
            params![prefix],
        )
        .map_err(|e| format!("seed number_prefix: {e}"))?;
        conn.execute(
            "INSERT OR IGNORE INTO board_meta(key, value) VALUES('issue_counter', '0')",
            [],
        )
        .map_err(|e| format!("seed issue_counter: {e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
            asset_signing: std::sync::OnceLock::new(),
        })
    }

    /// 注入 dispatch 签发上下文（gateway 装配时 set 一次；重复 set 忽略）。
    pub fn set_asset_signing(&self, ctx: crate::asset_token::AssetSignContext) {
        let _ = self.asset_signing.set(ctx);
    }

    /// dispatch 签发上下文（未注入 → None，派发 prompt 不带资产段）。
    pub fn asset_signing(&self) -> Option<crate::asset_token::AssetSignContext> {
        self.asset_signing.get().cloned()
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    // -----------------------------------------------------------------------
    // Issue CRUD
    // -----------------------------------------------------------------------

    /// 建 issue：事务内分配编号 + 写 created 活动 + 订阅创建者（及被指派者）。
    pub fn create_issue(&self, new: NewIssue) -> Result<Issue, String> {
        if new.title.trim().is_empty() {
            return Err("issue title must not be empty".to_string());
        }
        if let Some(at) = &new.assignee
            && new.assignee_id.as_deref().unwrap_or("").trim().is_empty()
        {
            return Err(format!("assignee {} requires assignee_id", at));
        }

        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        // 编号：counter 自增（事务内串行，防并发重号）。
        tx.execute(
            "UPDATE board_meta SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)
             WHERE key = 'issue_counter'",
            [],
        )
        .map_err(|e| e.to_string())?;
        let counter: i64 = tx
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM board_meta WHERE key = 'issue_counter'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let prefix: String = tx
            .query_row(
                "SELECT value FROM board_meta WHERE key = 'number_prefix'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let number = format!("{}-{}", prefix, counter);

        let now = Self::now();
        let status = IssueStatus::Backlog.as_str();
        // Swarm M1：required_tags 落 JSON TEXT（空 = NULL，行上少一坨 "[]"）。
        let required_tags_json = if new.required_tags.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&new.required_tags).map_err(|e| e.to_string())?)
        };
        tx.execute(
            "INSERT INTO issue (number, title, description, status, priority,
                assignee_type, assignee_id, creator_type, creator_id,
                parent_issue_id, project_id, due_date, position,
                acceptance_criteria, origin_type, origin_id,
                required_role, required_tags, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?19)",
            params![
                number,
                new.title,
                new.description,
                status,
                new.priority,
                new.assignee.map(|a| a.as_str()),
                new.assignee_id,
                new.creator.kind,
                new.creator.id,
                new.parent_issue_id,
                new.project_id,
                new.due_date,
                counter, // position 默认 = counter：先建在前，稳定排序
                new.acceptance_criteria,
                new.origin.as_ref().map(|o| o.origin_type.as_str()),
                new.origin.as_ref().map(|o| o.origin_id.as_str()),
                new.required_role.as_deref().filter(|s| !s.trim().is_empty()),
                required_tags_json,
                now,
            ],
        )
        .map_err(|e| e.to_string())?;
        let id = tx.last_insert_rowid();

        insert_activity(
            &tx,
            id,
            &new.creator,
            "created",
            Some(&format!("issue {number} created")),
            now,
        )?;
        insert_subscriber(&tx, id, &new.creator, "creator")?;
        if let (Some(at), Some(aid)) = (&new.assignee, &new.assignee_id) {
            insert_activity(
                &tx,
                id,
                &new.creator,
                "assigned",
                Some(&format!("{} → {at}/{aid}", new.creator.id)),
                now,
            )?;
            insert_subscriber(&tx, id, &Actor::new(at.as_str(), aid), "assignee")?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        // 先释放连接锁再走 get_issue（其内部会重新 lock 同一把 Mutex——
        // 持锁重入即死锁，见 nemesis-data 同类教训）。
        drop(conn);

        self.get_issue(id)
    }

    /// 按 id 取 issue；不存在报错。
    pub fn get_issue(&self, id: i64) -> Result<Issue, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT * FROM issue WHERE id = ?1",
            params![id],
            row_to_issue,
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("issue {id} not found"))
    }

    /// 按编号取（如 `NB-42`）；不存在报错。
    pub fn get_issue_by_number(&self, number: &str) -> Result<Issue, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT * FROM issue WHERE number = ?1",
            params![number],
            row_to_issue,
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("issue {number} not found"))
    }

    // -----------------------------------------------------------------------
    // 依赖表（Swarm M1：批内拆解依赖边；补派触发器的双向查询）
    // -----------------------------------------------------------------------

    /// 整体替换 issue 的依赖边（planner 落库路径；空切片 = 清空）。
    /// 引用不存在的 issue id 由 FK 约束诚实拒绝。
    pub fn set_issue_dependencies(&self, issue_id: i64, depends_on: &[i64]) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM issue_dependency WHERE issue_id = ?1", params![issue_id])
            .map_err(|e| e.to_string())?;
        for &dep in depends_on {
            tx.execute(
                "INSERT OR IGNORE INTO issue_dependency (issue_id, depends_on) VALUES (?1, ?2)",
                params![issue_id, dep],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// 本 issue 依赖哪些 issue（派发闸：依赖未 done 不派出）。
    pub fn dependencies_of(&self, issue_id: i64) -> Result<Vec<i64>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT depends_on FROM issue_dependency WHERE issue_id = ?1 ORDER BY depends_on")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![issue_id], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// 哪些 issue 依赖本 issue（补派触发器：X done 后扫它的 dependents）。
    pub fn dependents_of(&self, issue_id: i64) -> Result<Vec<i64>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT issue_id FROM issue_dependency WHERE depends_on = ?1 ORDER BY issue_id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![issue_id], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// 列出某父单的全部子单（创建顺序；无子单返回空 vec）。
    pub fn list_children(&self, parent_id: i64) -> Result<Vec<Issue>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT * FROM issue WHERE parent_issue_id = ?1 ORDER BY id ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![parent_id], row_to_issue)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// 列表（动态 WHERE + 稳定排序：position ASC, id DESC）。
    pub fn list_issues(&self, filter: &IssueFilter) -> Result<Vec<Issue>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut sql = String::from("SELECT * FROM issue WHERE 1=1");
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(st) = &filter.status {
            sql.push_str(&format!(" AND status = ?{}", args.len() + 1));
            args.push(Box::new(st.as_str().to_string()));
        }
        if let Some((at, aid)) = &filter.assignee {
            sql.push_str(&format!(
                " AND assignee_type = ?{} AND assignee_id = ?{}",
                args.len() + 1,
                args.len() + 2
            ));
            args.push(Box::new(at.as_str().to_string()));
            args.push(Box::new(aid.clone()));
        }
        if let Some(pid) = filter.project_id {
            sql.push_str(&format!(" AND project_id = ?{}", args.len() + 1));
            args.push(Box::new(pid));
        }
        if let Some(pri) = filter.priority {
            sql.push_str(&format!(" AND priority = ?{}", args.len() + 1));
            args.push(Box::new(pri));
        }
        let q = filter.query.as_deref().unwrap_or("").trim().to_string();
        if !q.is_empty() {
            sql.push_str(&format!(
                " AND (number LIKE ?{} OR title LIKE ?{})",
                args.len() + 1,
                args.len() + 1
            ));
            args.push(Box::new(format!("%{q}%")));
        }
        sql.push_str(" ORDER BY position ASC, id DESC");

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), row_to_issue)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// 字段级部分更新（status/assignee 不走这里——分别走 [`Self::transition_issue`]
    /// / [`Self::assign_issue`] 保证审计）。变更字段写 updated 活动。
    pub fn update_issue(
        &self,
        id: i64,
        patch: &IssuePatch,
        actor: &Actor,
    ) -> Result<Issue, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let old: Issue = tx
            .query_row(
                "SELECT * FROM issue WHERE id = ?1",
                params![id],
                row_to_issue,
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("issue {id} not found"))?;

        let mut changes: Vec<String> = Vec::new();
        let title = apply_patch(&mut changes, "title", &old.title, &patch.title);
        let description = apply_patch(
            &mut changes,
            "description",
            &old.description,
            &patch.description,
        );
        let priority = apply_patch(&mut changes, "priority", &old.priority, &patch.priority);
        let project_id = apply_set_opt(
            &mut changes,
            "project_id",
            &old.project_id,
            &patch.project_id,
        );
        let due_date = apply_set_opt(&mut changes, "due_date", &old.due_date, &patch.due_date);
        let position = apply_patch(&mut changes, "position", &old.position, &patch.position);
        let acceptance_criteria = apply_set_opt(
            &mut changes,
            "acceptance_criteria",
            &old.acceptance_criteria,
            &patch.acceptance_criteria,
        );
        let parent_issue_id = apply_set_opt(
            &mut changes,
            "parent_issue_id",
            &old.parent_issue_id,
            &patch.parent_issue_id,
        );

        let now = Self::now();
        tx.execute(
            "UPDATE issue SET title=?1, description=?2, priority=?3, project_id=?4,
                due_date=?5, position=?6, acceptance_criteria=?7, parent_issue_id=?8,
                updated_at=?9
             WHERE id=?10",
            params![
                title,
                description,
                priority,
                project_id,
                due_date,
                position,
                acceptance_criteria,
                parent_issue_id,
                now,
                id,
            ],
        )
        .map_err(|e| e.to_string())?;

        if !changes.is_empty() {
            insert_activity(
                &tx,
                id,
                actor,
                "updated",
                Some(&serde_json::to_string(&changes).unwrap_or_default()),
                now,
            )?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        drop(conn); // 释放锁再 get_issue（防持锁重入死锁）
        self.get_issue(id)
    }

    /// 状态机转移（§1.1）：非法转移拒绝；合法转移写 `status_change` 评论 +
    /// activity_log。终态不可转出。
    pub fn transition_issue(
        &self,
        id: i64,
        to: IssueStatus,
        actor: &Actor,
    ) -> Result<Issue, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let old: Issue = tx
            .query_row(
                "SELECT * FROM issue WHERE id = ?1",
                params![id],
                row_to_issue,
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("issue {id} not found"))?;

        crate::state_machine::validate_transition(old.status, to)?;

        let now = Self::now();
        tx.execute(
            "UPDATE issue SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![to.as_str(), now, id],
        )
        .map_err(|e| e.to_string())?;

        let note = format!("{} → {}", old.status, to);
        insert_comment(&tx, id, actor, &note, None, CommentType::StatusChange, now)?;
        insert_activity(
            &tx,
            id,
            actor,
            "status_changed",
            Some(
                &serde_json::json!({ "from": old.status.as_str(), "to": to.as_str() }).to_string(),
            ),
            now,
        )?;
        // 站内通知（W2 P3）：指派对象（非操作者本人）收到状态变化——状态在
        // 看板列可见，订阅者不推（避免 worker 回报写回时对创建者双份轰炸：
        // 评论通知已覆盖）。
        if let (Some(at), Some(aid)) = (&old.assignee, &old.assignee_id) {
            let assignee_actor = Actor::new(at.as_str(), aid);
            if assignee_actor != *actor {
                insert_notification(
                    &tx,
                    &NewNotification {
                        recipient: assignee_actor,
                        kind: notification_kind::STATUS_CHANGED.to_string(),
                        title: format!("{} {}", old.number, old.title),
                        content: note.clone(),
                        issue_id: Some(id),
                    },
                    now,
                )?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        drop(conn); // 释放锁再 get_issue（防持锁重入死锁）
        self.get_issue(id)
    }

    /// 指派 / 改派 / 清空指派（`assignee = None` 清空）。写 assigned 活动 +
    /// 自动订阅被指派者。
    pub fn assign_issue(
        &self,
        id: i64,
        assignee: Option<AssignmentType>,
        assignee_id: Option<String>,
        actor: &Actor,
    ) -> Result<Issue, String> {
        if let Some(at) = assignee {
            let aid = assignee_id.as_deref().unwrap_or("").trim();
            if aid.is_empty() {
                return Err(format!("assignee {at} requires assignee_id"));
            }
        }
        if assignee.is_none() != assignee_id.is_none() {
            return Err("assignee and assignee_id must be set together".to_string());
        }

        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let old: Issue = tx
            .query_row(
                "SELECT * FROM issue WHERE id = ?1",
                params![id],
                row_to_issue,
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("issue {id} not found"))?;

        let now = Self::now();
        tx.execute(
            "UPDATE issue SET assignee_type = ?1, assignee_id = ?2, updated_at = ?3 WHERE id = ?4",
            params![assignee.map(|a| a.as_str()), assignee_id, now, id,],
        )
        .map_err(|e| e.to_string())?;

        let detail = match (&assignee, &assignee_id) {
            (Some(at), Some(aid)) => format!("{at}/{aid}"),
            _ => "（清空）".to_string(),
        };
        insert_activity(
            &tx,
            id,
            actor,
            "assigned",
            Some(&format!(
                "{}: {} → {detail}",
                actor.id,
                display_assignee(&old)
            )),
            now,
        )?;
        if let (Some(at), Some(aid)) = (&assignee, &assignee_id) {
            let new_assignee = Actor::new(at.as_str(), aid);
            insert_subscriber(&tx, id, &new_assignee, "assignee")?;
            // 站内通知（W2 P3）：指派真的变了且不是自己指自己 → 通知被指派人。
            let changed = old.assignee != assignee || old.assignee_id != assignee_id;
            if changed && new_assignee != *actor {
                insert_notification(
                    &tx,
                    &NewNotification {
                        recipient: new_assignee,
                        kind: notification_kind::ASSIGNED.to_string(),
                        title: format!("{} {}", old.number, old.title),
                        content: format!("{}/{} 把任务指派给了你", actor.kind, actor.id),
                        issue_id: Some(id),
                    },
                    now,
                )?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        drop(conn); // 释放锁再 get_issue（防持锁重入死锁）
        self.get_issue(id)
    }

    /// 看板拖拽（W2 P3）：原子地完成「状态转移 + 列内排序」——同列重排只改
    /// position（不触发状态机，写 reordered 活动）；跨列拖动走状态机校验并写
    /// status_change 评论 + 指派对象通知（与 [`Self::transition_issue`] 同套
    /// 审计/通知语义，只是把两次写合并进一个事务）。
    pub fn move_issue(
        &self,
        id: i64,
        to: IssueStatus,
        position: i64,
        actor: &Actor,
    ) -> Result<Issue, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let old: Issue = tx
            .query_row(
                "SELECT * FROM issue WHERE id = ?1",
                params![id],
                row_to_issue,
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("issue {id} not found"))?;

        let status_changed = old.status != to;
        if status_changed {
            crate::state_machine::validate_transition(old.status, to)?;
        }

        let now = Self::now();
        tx.execute(
            "UPDATE issue SET status = ?1, position = ?2, updated_at = ?3 WHERE id = ?4",
            params![to.as_str(), position, now, id],
        )
        .map_err(|e| e.to_string())?;

        if status_changed {
            let note = format!("{} → {}", old.status, to);
            insert_comment(&tx, id, actor, &note, None, CommentType::StatusChange, now)?;
            insert_activity(
                &tx,
                id,
                actor,
                "status_changed",
                Some(
                    &serde_json::json!({
                        "from": old.status.as_str(),
                        "to": to.as_str(),
                        "position": position,
                    })
                    .to_string(),
                ),
                now,
            )?;
            if let (Some(at), Some(aid)) = (&old.assignee, &old.assignee_id) {
                let assignee_actor = Actor::new(at.as_str(), aid);
                if assignee_actor != *actor {
                    insert_notification(
                        &tx,
                        &NewNotification {
                            recipient: assignee_actor,
                            kind: notification_kind::STATUS_CHANGED.to_string(),
                            title: format!("{} {}", old.number, old.title),
                            content: note,
                            issue_id: Some(id),
                        },
                        now,
                    )?;
                }
            }
        } else {
            insert_activity(
                &tx,
                id,
                actor,
                "reordered",
                Some(&serde_json::json!({ "position": position }).to_string()),
                now,
            )?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        drop(conn); // 释放锁再 get_issue（防持锁重入死锁）
        self.get_issue(id)
    }

    // -----------------------------------------------------------------------
    // 评论 / 时间线 / 订阅
    // -----------------------------------------------------------------------

    /// 加评论（订阅作者 + commented 活动 + 站内通知：普通评论通知
    /// （订阅者 ∪ 指派 − 作者），@提及优先；status_change/system 评论
    /// 不通知——状态转移自有通知，系统写回由调用方决定）。
    pub fn add_comment(&self, new: NewComment) -> Result<Comment, String> {
        if new.content.trim().is_empty() {
            return Err("comment content must not be empty".to_string());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        add_comment_on(&conn, &new)
    }

    /// 评论列表（issue 内按时间升序，线程展开由前端做）。
    pub fn list_comments(&self, issue_id: i64) -> Result<Vec<Comment>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT * FROM comment WHERE issue_id = ?1 ORDER BY id ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![issue_id], row_to_comment)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// 时间线（activity_log，升序）。
    pub fn list_activity(&self, issue_id: i64) -> Result<Vec<ActivityLog>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT * FROM activity_log WHERE issue_id = ?1 ORDER BY id ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![issue_id], row_to_activity)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// 订阅（幂等；reason 覆盖更新）。
    pub fn subscribe(&self, issue_id: i64, who: &Actor, reason: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        insert_subscriber(&conn, issue_id, who, reason)
    }

    /// 退订（不存在静默成功——退订是幂等意图）。
    pub fn unsubscribe(&self, issue_id: i64, who: &Actor) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM issue_subscriber
             WHERE issue_id = ?1 AND subscriber_type = ?2 AND subscriber_id = ?3",
            params![issue_id, who.kind, who.id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn list_subscribers(&self, issue_id: i64) -> Result<Vec<Subscriber>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT issue_id, subscriber_type, subscriber_id, reason
                 FROM issue_subscriber WHERE issue_id = ?1 ORDER BY subscriber_id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![issue_id], |r| {
                Ok(Subscriber {
                    issue_id: r.get(0)?,
                    subscriber: Actor::new(&r.get::<_, String>(1)?, &r.get::<_, String>(2)?),
                    reason: r.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    // -----------------------------------------------------------------------
    // 项目
    // -----------------------------------------------------------------------

    pub fn create_project(
        &self,
        name: &str,
        description: &str,
        lead: Option<&Actor>,
        icon: &str,
    ) -> Result<Project, String> {
        if name.trim().is_empty() {
            return Err("project name must not be empty".to_string());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = Self::now();
        conn.execute(
            "INSERT INTO project (name, description, status, priority, lead_type, lead_id, icon, created_at)
             VALUES (?1, ?2, 'active', 1, ?3, ?4, ?5, ?6)",
            params![
                name,
                description,
                lead.as_ref().map(|l| l.kind.as_str()),
                lead.as_ref().map(|l| l.id.as_str()),
                icon,
                now,
            ],
        )
        .map_err(|e| format!("create_project: {e}"))?;
        let id = conn.last_insert_rowid();
        drop(conn); // 释放锁再 get_project（防持锁重入死锁）
        self.get_project(id)
    }

    pub fn get_project(&self, id: i64) -> Result<Project, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT * FROM project WHERE id = ?1",
            params![id],
            row_to_project,
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("project {id} not found"))
    }

    /// 字段级部分更新项目（None = 不改；改名撞 UNIQUE 约束时报错透传）。
    /// 归档走 `status = "archived"`（软删除——列表仍可见）。
    pub fn update_project(&self, id: i64, patch: &ProjectPatch) -> Result<Project, String> {
        if let Some(n) = &patch.name
            && n.trim().is_empty()
        {
            return Err("project name must not be empty".to_string());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let old: Project = conn
            .query_row(
                "SELECT * FROM project WHERE id = ?1",
                params![id],
                row_to_project,
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("project {id} not found"))?;
        let name = patch.name.clone().unwrap_or(old.name);
        let description = patch.description.clone().unwrap_or(old.description);
        let status = patch.status.clone().unwrap_or(old.status);
        let icon = patch.icon.clone().unwrap_or(old.icon);
        conn.execute(
            "UPDATE project SET name = ?1, description = ?2, status = ?3, icon = ?4 WHERE id = ?5",
            params![name, description, status, icon, id],
        )
        .map_err(|e| format!("update_project: {e}"))?;
        drop(conn); // 释放锁再 get_project（防持锁重入死锁）
        self.get_project(id)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT * FROM project ORDER BY id ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], row_to_project)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    // -----------------------------------------------------------------------
    // 附件（P1：仅元数据）
    // -----------------------------------------------------------------------

    pub fn add_attachment(
        &self,
        issue_id: i64,
        filename: &str,
        storage_path: &str,
        size: i64,
    ) -> Result<Attachment, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = Self::now();
        conn.execute(
            "INSERT INTO attachment (issue_id, filename, storage_path, size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![issue_id, filename, storage_path, size, now],
        )
        .map_err(|e| e.to_string())?;
        let id = conn.last_insert_rowid();
        Ok(Attachment {
            id,
            issue_id,
            filename: filename.to_string(),
            storage_path: storage_path.to_string(),
            size,
            created_at: now,
        })
    }

    pub fn list_attachments(&self, issue_id: i64) -> Result<Vec<Attachment>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT * FROM attachment WHERE issue_id = ?1 ORDER BY id ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![issue_id], |r| {
                Ok(Attachment {
                    id: r.get(0)?,
                    issue_id: r.get(1)?,
                    filename: r.get(2)?,
                    storage_path: r.get(3)?,
                    size: r.get(4)?,
                    created_at: r.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// 按 id 取附件元数据；不存在报错（attachment.get 下载入口用）。
    pub fn get_attachment(&self, id: i64) -> Result<Attachment, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT * FROM attachment WHERE id = ?1", params![id], |r| {
            Ok(Attachment {
                id: r.get(0)?,
                issue_id: r.get(1)?,
                filename: r.get(2)?,
                storage_path: r.get(3)?,
                size: r.get(4)?,
                created_at: r.get(5)?,
            })
        })
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("attachment {id} not found"))
    }

    // -----------------------------------------------------------------------
    // 通知 / 收件箱（W2 P3；经 21 通道的站外投递留 P4）
    // -----------------------------------------------------------------------

    /// 发一条站内通知（事件钩子之外的显式入口——store 内部的
    /// assigned/commented/mentioned/status_changed 钩子不走这里）。
    pub fn notify(&self, n: NewNotification) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        insert_notification(&conn, &n, Self::now())
    }

    /// 派发事件显式通知（W2 P4 超时 sweep / 离线判定用；kind 取
    /// [`notification_kind`]，如 `DISPATCH_FAILED`）：收件人 = 创建者 ∪
    /// 指派 ∪ 订阅者（去重）。与 add_comment 的评论通知（订阅者 ∪ 指派 −
    /// 作者）不同——失败要保证创建者一定收到（创建者可能未订阅）。
    pub fn notify_dispatch_event(
        &self,
        issue_id: i64,
        kind: &str,
        content: &str,
    ) -> Result<(), String> {
        // 先读 issue（锁外调 get_issue，防持锁重入死锁）。
        let issue = self.get_issue(issue_id)?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let issue_title = format!("{} {}", issue.number, issue.title);
        let mut recipients = self_subscribers(&conn, issue_id)?;
        if !recipients.iter().any(|r| r == &issue.creator) {
            recipients.push(issue.creator.clone());
        }
        if let (Some(at), Some(aid)) = (&issue.assignee, &issue.assignee_id) {
            let a = Actor::new(at.as_str(), aid);
            if !recipients.iter().any(|r| r == &a) {
                recipients.push(a);
            }
        }
        let now = Self::now();
        for r in &recipients {
            insert_notification(
                &conn,
                &NewNotification {
                    recipient: r.clone(),
                    kind: kind.to_string(),
                    title: issue_title.clone(),
                    content: content.to_string(),
                    issue_id: Some(issue_id),
                },
                now,
            )?;
        }
        Ok(())
    }

    /// 收件箱列表（created_at 降序）。`recipient_id = None` → 该类型的全部
    /// 收件人（dashboard 单管理员收件箱语义：admin 通知全员可见；
    /// agent 收件人按节点 id 精确过滤，供 P4 通道投递用）。
    pub fn list_notifications(
        &self,
        recipient_type: &str,
        recipient_id: Option<&str>,
        unread_only: bool,
        limit: usize,
    ) -> Result<Vec<Notification>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut sql = String::from("SELECT * FROM notification WHERE recipient_type = ?1");
        if recipient_id.is_some() {
            sql.push_str(" AND recipient_id = ?2");
        }
        if unread_only {
            sql.push_str(" AND read = 0");
        }
        sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(recipient_type.to_string())];
        if let Some(rid) = recipient_id {
            args.push(Box::new(rid.to_string()));
        }
        args.push(Box::new(limit as i64));
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), row_to_notification)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// 标记单条已读；返回是否真的改变了状态（幂等：已读重复标记 = false）。
    pub fn mark_notification_read(&self, id: i64) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n = conn
            .execute(
                "UPDATE notification SET read = 1 WHERE id = ?1 AND read = 0",
                params![id],
            )
            .map_err(|e| e.to_string())?;
        Ok(n > 0)
    }

    /// 全部已读（按收件类型 [+ 精确 id]）；返回标记条数。
    pub fn mark_all_notifications_read(
        &self,
        recipient_type: &str,
        recipient_id: Option<&str>,
    ) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n = match recipient_id {
            Some(rid) => conn
                .execute(
                    "UPDATE notification SET read = 1
                     WHERE read = 0 AND recipient_type = ?1 AND recipient_id = ?2",
                    params![recipient_type, rid],
                )
                .map_err(|e| e.to_string())?,
            None => conn
                .execute(
                    "UPDATE notification SET read = 1 WHERE read = 0 AND recipient_type = ?1",
                    params![recipient_type],
                )
                .map_err(|e| e.to_string())?,
        };
        Ok(n)
    }

    /// 未读数（收件箱角标）。
    pub fn unread_notification_count(
        &self,
        recipient_type: &str,
        recipient_id: Option<&str>,
    ) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n: i64 = match recipient_id {
            Some(rid) => conn
                .query_row(
                    "SELECT COUNT(*) FROM notification
                     WHERE read = 0 AND recipient_type = ?1 AND recipient_id = ?2",
                    params![recipient_type, rid],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?,
            None => conn
                .query_row(
                    "SELECT COUNT(*) FROM notification WHERE read = 0 AND recipient_type = ?1",
                    params![recipient_type],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?,
        };
        Ok(n)
    }

    // -----------------------------------------------------------------------
    // 派发（W2 P2：issue ↔ peer_chat task 绑定与写回）
    // -----------------------------------------------------------------------

    /// 登记派发：task_id ↔ issue 绑定 + `dispatched` 活动（审计痕迹与登记
    /// 同事务；重复 task_id 拒绝——一个 task 只挂一个 issue）。
    pub fn insert_dispatch(
        &self,
        task_id: &str,
        issue_id: i64,
        worker_id: &str,
        actor: &Actor,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = Self::now();
        conn.execute(
            "INSERT INTO issue_dispatch (task_id, issue_id, worker_id, state, dispatched_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                task_id,
                issue_id,
                worker_id,
                dispatch_state::DISPATCHED,
                now
            ],
        )
        .map_err(|e| format!("insert_dispatch: {e}"))?;
        insert_activity(
            &conn,
            issue_id,
            actor,
            "dispatched",
            Some(&serde_json::json!({ "task_id": task_id, "worker_id": worker_id }).to_string()),
            now,
        )?;
        Ok(())
    }

    /// 按 task_id 查派发记录（peer_chat_callback 写回路由用）。
    pub fn get_dispatch(&self, task_id: &str) -> Result<Option<DispatchRecord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT task_id, issue_id, worker_id, state, dispatched_at, completed_at
             FROM issue_dispatch WHERE task_id = ?1",
            params![task_id],
            row_to_dispatch,
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// issue 的派发历史（时间升序；UI 展示）。
    pub fn list_dispatches(&self, issue_id: i64) -> Result<Vec<DispatchRecord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT task_id, issue_id, worker_id, state, dispatched_at, completed_at
                 FROM issue_dispatch WHERE issue_id = ?1 ORDER BY dispatched_at ASC, task_id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![issue_id], row_to_dispatch)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// issue 是否有未完结（`dispatched`）派发（防重复派发）。
    pub fn has_active_dispatch(&self, issue_id: i64) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM issue_dispatch WHERE issue_id = ?1 AND state = ?2",
                params![issue_id, dispatch_state::DISPATCHED],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(n > 0)
    }

    /// 按目标 worker 统计未完结（`dispatched`）派发数（Swarm M1 匹配器
    /// 负载输入：同分节点选更闲者）。无派发的 worker 不在返回表里（调用方
    /// 按 0 处理）。
    pub fn count_active_dispatch_by_worker(&self) -> Result<std::collections::HashMap<String, usize>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT worker_id, COUNT(*) FROM issue_dispatch
                 WHERE state = ?1 GROUP BY worker_id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![dispatch_state::DISPATCHED], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))
            })
            .map_err(|e| e.to_string())?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (worker, count) = row.map_err(|e| e.to_string())?;
            map.insert(worker, count);
        }
        Ok(map)
    }

    /// 终结派发：`done` / `failed`（P4 扩展 cancelled/timeout）。
    /// 只有 `dispatched` 态可终结——返回 `Ok(true)` 表示本次调用完成了终结
    /// （幂等：重复回调拿到 `Ok(false)`，写回方据此跳过重复评论/转移）。
    pub fn finish_dispatch(&self, task_id: &str, state: &str) -> Result<bool, String> {
        if state != dispatch_state::DONE && state != dispatch_state::FAILED {
            return Err(format!(
                "invalid dispatch state: {state}（可选 done/failed）"
            ));
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n = conn
            .execute(
                "UPDATE issue_dispatch
                 SET state = ?2, completed_at = ?3
                 WHERE task_id = ?1 AND state = ?4",
                params![task_id, state, Self::now(), dispatch_state::DISPATCHED],
            )
            .map_err(|e| e.to_string())?;
        Ok(n > 0)
    }

    /// issue 当前活跃（`dispatched`）派发记录（P4 cancel 入口：拿到 task_id
    /// 才能下行 cancel，同时确认该 issue 确有在途派发；多条取最新）。
    pub fn get_active_dispatch(&self, issue_id: i64) -> Result<Option<DispatchRecord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT task_id, issue_id, worker_id, state, dispatched_at, completed_at
             FROM issue_dispatch WHERE issue_id = ?1 AND state = ?2
             ORDER BY dispatched_at DESC, task_id DESC LIMIT 1",
            params![issue_id, dispatch_state::DISPATCHED],
            row_to_dispatch,
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// 全部在途派发（P4 超时 sweep 扫描用；时间升序——最老的先处理）。
    pub fn list_active_dispatches(&self) -> Result<Vec<DispatchRecord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT task_id, issue_id, worker_id, state, dispatched_at, completed_at
                 FROM issue_dispatch WHERE state = ?1 ORDER BY dispatched_at ASC, task_id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![dispatch_state::DISPATCHED], row_to_dispatch)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// 管理端取消派发（P4 per-task cancel 的 A 侧落账）：state → cancelled +
    /// `dispatch_cancelled` 活动（同事务）。只有 `dispatched` 态可取消——
    /// 返回 `Ok(Some(record))` 表示本次调用赢得竞态（调用方据此才下行
    /// task_cancel RPC）；`Ok(None)` = 已终结（写回回调 / 超时 sweep 先到），
    /// 幂等跳过、不写活动。
    pub fn cancel_dispatch(
        &self,
        task_id: &str,
        actor: &Actor,
    ) -> Result<Option<DispatchRecord>, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let now = Self::now();
        let n = tx
            .execute(
                "UPDATE issue_dispatch
                 SET state = ?2, completed_at = ?3
                 WHERE task_id = ?1 AND state = ?4",
                params![
                    task_id,
                    dispatch_state::CANCELLED,
                    now,
                    dispatch_state::DISPATCHED
                ],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None); // 已终结——竞态输了，无活动可写
        }
        let issue_id: i64 = tx
            .query_row(
                "SELECT issue_id FROM issue_dispatch WHERE task_id = ?1",
                params![task_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        insert_activity(
            &tx,
            issue_id,
            actor,
            "dispatch_cancelled",
            Some(&serde_json::json!({ "task_id": task_id }).to_string()),
            now,
        )?;
        tx.commit().map_err(|e| e.to_string())?;
        drop(conn); // 释放锁再 get_dispatch（防持锁重入死锁）
        self.get_dispatch(task_id)
    }

    /// 超时兜底终结（P4 sweep）：state → failed + `dispatch_timeout` 活动。
    /// `WHERE state = 'dispatched'` 守卫与写回回调竞态——`Ok(Some(record))` =
    /// 本次调用赢得竞态（调用方负责 ⛔ System 评论 + dispatch_failed 通知）；
    /// `Ok(None)` = 回调已先终结，跳过。
    pub fn fail_dispatch(
        &self,
        task_id: &str,
        details: &str,
    ) -> Result<Option<DispatchRecord>, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let now = Self::now();
        let n = tx
            .execute(
                "UPDATE issue_dispatch
                 SET state = ?2, completed_at = ?3
                 WHERE task_id = ?1 AND state = ?4",
                params![
                    task_id,
                    dispatch_state::FAILED,
                    now,
                    dispatch_state::DISPATCHED
                ],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None); // 写回回调先到——竞态输了
        }
        let issue_id: i64 = tx
            .query_row(
                "SELECT issue_id FROM issue_dispatch WHERE task_id = ?1",
                params![task_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let actor = Actor::system("board");
        insert_activity(
            &tx,
            issue_id,
            &actor,
            "dispatch_timeout",
            Some(&serde_json::json!({ "task_id": task_id, "details": details }).to_string()),
            now,
        )?;
        tx.commit().map_err(|e| e.to_string())?;
        drop(conn); // 释放锁再 get_dispatch（防持锁重入死锁）
        self.get_dispatch(task_id)
    }

    // -----------------------------------------------------------------------
    // 自动化（W2 P4：autopilot 规则 CRUD + run 簿记）
    // -----------------------------------------------------------------------

    /// 建自动化规则。cron/title/name 非空校验在此；cron 表达式本身的合法性
    /// 由 handler 层经 `nemesis-cron` 的 validate_schedule 校验（store 不依赖
    /// nemesis-cron）。
    pub fn create_autopilot(&self, n: &NewAutopilot) -> Result<Autopilot, String> {
        if n.name.trim().is_empty() {
            return Err("autopilot name must not be empty".to_string());
        }
        if n.title.trim().is_empty() {
            return Err("autopilot title must not be empty".to_string());
        }
        if n.cron.trim().is_empty() {
            return Err("autopilot cron must not be empty".to_string());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = Self::now();
        conn.execute(
            "INSERT INTO autopilot
             (name, cron, title, description, priority, project_id, target, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                n.name,
                n.cron,
                n.title,
                n.description,
                n.priority,
                n.project_id,
                n.target,
                n.enabled as i64,
                now,
            ],
        )
        .map_err(|e| format!("create_autopilot: {e}"))?;
        let id = conn.last_insert_rowid();
        drop(conn); // 释放锁再 get_autopilot（防持锁重入死锁）
        self.get_autopilot(id)
    }

    pub fn get_autopilot(&self, id: i64) -> Result<Autopilot, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT * FROM autopilot WHERE id = ?1",
            params![id],
            row_to_autopilot,
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("autopilot {id} not found"))
    }

    pub fn list_autopilots(&self) -> Result<Vec<Autopilot>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT * FROM autopilot ORDER BY id ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], row_to_autopilot)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// 字段级部分更新（None = 不改；空串 title/name 拒绝）。
    pub fn update_autopilot(&self, id: i64, patch: &AutopilotPatch) -> Result<Autopilot, String> {
        if let Some(n) = &patch.name
            && n.trim().is_empty()
        {
            return Err("autopilot name must not be empty".to_string());
        }
        if let Some(t) = &patch.title
            && t.trim().is_empty()
        {
            return Err("autopilot title must not be empty".to_string());
        }
        if let Some(c) = &patch.cron
            && c.trim().is_empty()
        {
            return Err("autopilot cron must not be empty".to_string());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let old: Autopilot = conn
            .query_row(
                "SELECT * FROM autopilot WHERE id = ?1",
                params![id],
                row_to_autopilot,
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("autopilot {id} not found"))?;

        let name = patch.name.clone().unwrap_or(old.name);
        let cron = patch.cron.clone().unwrap_or(old.cron);
        let title = patch.title.clone().unwrap_or(old.title);
        let description = patch.description.clone().unwrap_or(old.description);
        let priority = patch.priority.unwrap_or(old.priority);
        let project_id = patch.project_id.or(old.project_id);
        let target = patch.target.clone().unwrap_or(old.target);
        let enabled = patch.enabled.unwrap_or(old.enabled);
        conn.execute(
            "UPDATE autopilot
             SET name = ?2, cron = ?3, title = ?4, description = ?5, priority = ?6,
                 project_id = ?7, target = ?8, enabled = ?9, updated_at = ?10
             WHERE id = ?1",
            params![
                id,
                name,
                cron,
                title,
                description,
                priority,
                project_id,
                target,
                enabled as i64,
                Self::now(),
            ],
        )
        .map_err(|e| format!("update_autopilot: {e}"))?;
        drop(conn); // 释放锁再 get_autopilot（防持锁重入死锁）
        self.get_autopilot(id)
    }

    /// 删除规则（run 历史 = origin=autopilot/{id} 的 issue，不随删）。
    /// 返回是否真的删了（幂等：重复删 = false）。
    pub fn remove_autopilot(&self, id: i64) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n = conn
            .execute("DELETE FROM autopilot WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(n > 0)
    }

    /// 回存 / 清除 live CronService job id（注册/摘除 job 后的簿记写）。
    pub fn set_autopilot_cron_job(&self, id: i64, cron_job_id: Option<&str>) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n = conn
            .execute(
                "UPDATE autopilot SET cron_job_id = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, cron_job_id, Self::now()],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!("autopilot {id} not found"));
        }
        Ok(())
    }

    /// 触发落账：last_run_at = now。
    pub fn mark_autopilot_run(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n = conn
            .execute(
                "UPDATE autopilot SET last_run_at = ?2, updated_at = ?2 WHERE id = ?1",
                params![id, Self::now()],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!("autopilot {id} not found"));
        }
        Ok(())
    }

    /// run 历史：按 origin（autopilot/{id}）建的 issue，时间降序截断
    /// （autopilot.runs 面板数据源）。
    pub fn list_issues_by_origin(
        &self,
        origin_type: &str,
        origin_id: &str,
        limit: usize,
    ) -> Result<Vec<Issue>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM issue
                 WHERE origin_type = ?1 AND origin_id = ?2
                 ORDER BY created_at DESC, id DESC LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![origin_type, origin_id, limit as i64], row_to_issue)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    // -----------------------------------------------------------------------
    // 统计（看板列计数 / P1 验收）
    // -----------------------------------------------------------------------

    /// 按状态计数的 issue 数（看板列头徽标）。
    pub fn count_by_status(&self) -> Result<Vec<(IssueStatus, i64)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT status, COUNT(*) FROM issue GROUP BY status")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                let s: String = r.get(0)?;
                let n: i64 = r.get(1)?;
                IssueStatus::from_str(&s).map(|st| (st, n)).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        format!("unknown status {s}").into(),
                    )
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    // -----------------------------------------------------------------------
    // 讨论频道 + 任务资产（Swarm M2；impl-plan §4.1/§4.2）
    // -----------------------------------------------------------------------

    /// master 启动装配时 ensure 三个默认频道（`#dev` / `#qa` / `#general`），
    /// `INSERT OR IGNORE` 幂等（§4.2.1）。
    pub fn ensure_default_channels(&self) -> Result<(), String> {
        for name in ["#dev", "#qa", "#general"] {
            self.conn
                .lock()
                .map_err(|e| e.to_string())?
                .execute(
                    "INSERT OR IGNORE INTO channel (name, topic, created_at) VALUES (?1, '', ?2)",
                    params![name, Self::now()],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// 建频道；名归一化（缺 `#` 前缀自动补），重名报错。
    pub fn create_channel(&self, new: NewChannel) -> Result<Channel, String> {
        let name = normalize_channel_name(&new.name)?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = Self::now();
        conn.execute(
            "INSERT INTO channel (name, topic, created_at) VALUES (?1, ?2, ?3)",
            params![name, new.topic, now],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                format!("channel {name} already exists")
            } else {
                e.to_string()
            }
        })?;
        Ok(Channel {
            id: conn.last_insert_rowid(),
            name,
            topic: new.topic,
            created_at: now,
        })
    }

    /// 频道列表（建频道序）。
    pub fn list_channels(&self) -> Result<Vec<Channel>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, name, topic, created_at FROM channel ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Channel {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    topic: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// 按名取频道（输入同样归一化）。
    pub fn get_channel_by_name(&self, name: &str) -> Result<Option<Channel>, String> {
        let name = normalize_channel_name(name)?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT id, name, topic, created_at FROM channel WHERE name = ?1",
            params![name],
            |row| {
                Ok(Channel {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    topic: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// 成员入频道（幂等 upsert；频道不存在报错——FK 之外的显式校验）。
    pub fn join_channel(&self, channel_id: i64, member: Actor) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let exists: Option<i64> = conn
            .query_row(
                "SELECT id FROM channel WHERE id = ?1",
                params![channel_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if exists.is_none() {
            return Err(format!("channel {channel_id} not found"));
        }
        conn.execute(
            "INSERT OR IGNORE INTO channel_member
                 (channel_id, member_type, member_id, last_seen_message_id)
             VALUES (?1, ?2, ?3, 0)",
            params![channel_id, member.kind, member.id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 成员离频道（幂等；成员下线**不**调这个——回来还在，见 §4.2.1）。
    pub fn leave_channel(&self, channel_id: i64, member: &Actor) -> Result<(), String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute(
                "DELETE FROM channel_member
                 WHERE channel_id = ?1 AND member_type = ?2 AND member_id = ?3",
                params![channel_id, member.kind, member.id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 成员是否在任何频道有成员行（first-join 判据：跨全表零行 =
    /// 全新节点，才允许自动收编；手动 join 过任一频道或被管理员
    /// leave 出的成员不再被 announce 自动拉回）。
    pub fn has_any_channel_membership(&self, member: &Actor) -> Result<bool, String> {
        let n: i64 = self
            .conn
            .lock()
            .map_err(|e| e.to_string())?
            .query_row(
                "SELECT COUNT(*) FROM channel_member WHERE member_type = ?1 AND member_id = ?2",
                params![member.kind, member.id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(n > 0)
    }

    /// 上行发言统一落库入口（M3 `board.comment.post` 的存储侧；impl-plan
    /// §5.2① + G12 幂等）。单次持锁完成三步，对其他 store 调用方原子：
    /// ① `msg_dedup` 认领（同 `(origin_node, client_msg_id)` 重复 → 直接
    /// 返回首响，不重复落库）；② 落库（issue 评论走 [`add_comment_on`]
    /// 全副作用 / 频道消息走 [`append_channel_message_on`]）；③ `seq_ledger`
    /// 登记单调 seq。中途失败回滚认领行，同 id 重试不受影响。
    /// `kind_tag`：issue → `CommentType` 词（未知归 comment）；channel →
    /// `channel_message_type` 词。
    #[allow(clippy::too_many_arguments)]
    pub fn post_discussion_envelope(
        &self,
        origin_node: &str,
        client_msg_id: &str,
        target_kind: &str,
        target_id: i64,
        sender: &Actor,
        content: &str,
        reply_to: Option<i64>,
        kind_tag: &str,
    ) -> Result<PostedMessage, String> {
        if content.trim().is_empty() {
            return Err("content must not be empty".to_string());
        }
        if client_msg_id.trim().is_empty() {
            return Err("client_msg_id must not be empty".to_string());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = BoardStore::now();

        // ① 幂等认领：INSERT OR IGNORE 占位；changes()==0 → 重复请求。
        conn.execute(
            "INSERT OR IGNORE INTO msg_dedup (origin_node, client_msg_id, first_response, created_at)
             VALUES (?1, ?2, '', ?3)",
            params![origin_node, client_msg_id, now],
        )
        .map_err(|e| e.to_string())?;
        if conn.changes() == 0 {
            let cached: String = conn
                .query_row(
                    "SELECT first_response FROM msg_dedup
                     WHERE origin_node = ?1 AND client_msg_id = ?2",
                    params![origin_node, client_msg_id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            return Ok(PostedMessage {
                is_new: false,
                message_id: 0,
                seq: 0,
                response: serde_json::from_str(&cached).unwrap_or(serde_json::Value::Null),
            });
        }

        // ②③ 落库 + seq 登记；失败先删认领行（回滚），同 id 可重试。
        let posted = (|| -> Result<PostedMessage, String> {
            let message_id = match target_kind {
                thread_kind::ISSUE => {
                    let ctype = CommentType::from_str(kind_tag).unwrap_or(CommentType::Comment);
                    let comment = add_comment_on(
                        &conn,
                        &NewComment {
                            issue_id: target_id,
                            author: sender.clone(),
                            content: content.to_string(),
                            parent_id: reply_to,
                            ctype,
                        },
                    )?;
                    comment.id
                }
                thread_kind::CHANNEL => {
                    let msg = append_channel_message_on(
                        &conn,
                        &NewChannelMessage {
                            channel_id: target_id,
                            sender: sender.clone(),
                            content: content.to_string(),
                            parent_id: reply_to,
                            mtype: kind_tag.to_string(),
                        },
                    )?;
                    msg.id
                }
                other => return Err(format!("unknown thread kind: {other}")),
            };
            conn.execute(
                "INSERT INTO seq_ledger (thread_kind, thread_id, message_id, sender_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![target_kind, target_id, message_id, sender.id, now],
            )
            .map_err(|e| e.to_string())?;
            let seq = conn.last_insert_rowid();
            let response = if target_kind == thread_kind::ISSUE {
                serde_json::json!({"comment_id": message_id, "seq": seq})
            } else {
                serde_json::json!({"message_id": message_id, "seq": seq})
            };
            Ok(PostedMessage {
                is_new: true,
                message_id,
                seq,
                response,
            })
        })();

        match posted {
            Ok(p) => {
                conn.execute(
                    "UPDATE msg_dedup SET first_response = ?3
                     WHERE origin_node = ?1 AND client_msg_id = ?2",
                    params![origin_node, client_msg_id, p.response.to_string()],
                )
                .map_err(|e| e.to_string())?;
                Ok(p)
            }
            Err(e) => {
                let _ = conn.execute(
                    "DELETE FROM msg_dedup WHERE origin_node = ?1 AND client_msg_id = ?2",
                    params![origin_node, client_msg_id],
                );
                Err(e)
            }
        }
    }

    /// 幂等预检（M3 上行 handler 用）：`(origin_node, client_msg_id)` 已
    /// 认领过则返回缓存首响（None = 全新请求）。预检在额度扣账**之前**——
    /// 重复请求（网络重传）不消耗讨论额度。
    pub fn check_duplicate(
        &self,
        origin_node: &str,
        client_msg_id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let cached: Option<String> = conn
            .query_row(
                "SELECT first_response FROM msg_dedup
                 WHERE origin_node = ?1 AND client_msg_id = ?2",
                params![origin_node, client_msg_id],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other.to_string()),
            })?;
        Ok(cached.and_then(|s| serde_json::from_str(&s).ok()))
    }

    /// board.sync 补拉：`since_seq` 之后（不含）的台账行，按 seq 升序，
    /// `limit` 上限。join 原表取发送者/内容（台账只存路由键）。
    pub fn list_messages_since(&self, since_seq: i64, limit: i64) -> Result<Vec<LedgerEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT l.seq, l.thread_kind, l.thread_id, l.message_id,
                        COALESCE(c.author_type, m.sender_type, '') AS sender_type,
                        COALESCE(c.author_id, m.sender_id, '')      AS sender_id,
                        COALESCE(c.content, m.content, '')          AS content,
                        CASE WHEN l.thread_kind = 'issue' THEN c.parent_id
                             ELSE m.parent_id END                   AS parent_id,
                        COALESCE(c.ctype, m.mtype, '')              AS kind_tag,
                        COALESCE(c.created_at, m.created_at, l.created_at) AS created_at
                 FROM seq_ledger l
                 LEFT JOIN comment c
                     ON l.thread_kind = 'issue' AND c.id = l.message_id
                 LEFT JOIN channel_message m
                     ON l.thread_kind = 'channel' AND m.id = l.message_id
                 WHERE l.seq > ?1
                 ORDER BY l.seq ASC
                 LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![since_seq, limit], |row| {
                Ok(LedgerEntry {
                    seq: row.get(0)?,
                    thread_kind: row.get(1)?,
                    thread_id: row.get(2)?,
                    message_id: row.get(3)?,
                    sender_type: row.get(4)?,
                    sender_id: row.get(5)?,
                    content: row.get(6)?,
                    parent_id: row.get(7)?,
                    kind_tag: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// 当前全局最大 seq（worker 首次上线的 sync 基线；空表 = 0）。
    pub fn latest_seq(&self) -> Result<i64, String> {
        let n: i64 = self
            .conn
            .lock()
            .map_err(|e| e.to_string())?
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM seq_ledger",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(n)
    }

    /// 频道成员列表（裁决器路由查这张表，§4.2.1）。
    pub fn list_channel_members(&self, channel_id: i64) -> Result<Vec<ChannelMember>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT channel_id, member_type, member_id, last_seen_message_id
                 FROM channel_member WHERE channel_id = ?1 ORDER BY member_type, member_id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![channel_id], |row| {
                Ok(ChannelMember {
                    channel_id: row.get(0)?,
                    member: Actor::new(&row.get::<_, String>(1)?, &row.get::<_, String>(2)?),
                    last_seen_message_id: row.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// 推进未读游标（只前进；`after_id` 补拉语义的写侧）。
    pub fn mark_channel_seen(
        &self,
        channel_id: i64,
        member: &Actor,
        message_id: i64,
    ) -> Result<(), String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute(
                "UPDATE channel_member SET last_seen_message_id = MAX(last_seen_message_id, ?1)
                 WHERE channel_id = ?2 AND member_type = ?3 AND member_id = ?4",
                params![message_id, channel_id, member.kind, member.id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 追加频道消息（`mtype` 空串 = text）。
    pub fn append_channel_message(
        &self,
        new: NewChannelMessage,
    ) -> Result<ChannelMessage, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        append_channel_message_on(&conn, &new)
    }

    /// 增量拉取频道消息：`after_id` 游标语义（补拉与前端翻页同源，§4.1），
    /// id 升序，`limit` 上限。
    pub fn list_channel_messages(
        &self,
        channel_id: i64,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<ChannelMessage>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, channel_id, sender_type, sender_id, content, parent_id, mtype, created_at
                 FROM channel_message
                 WHERE channel_id = ?1 AND id > ?2
                 ORDER BY id ASC LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![channel_id, after_id, limit], |row| {
                Ok(ChannelMessage {
                    id: row.get(0)?,
                    channel_id: row.get(1)?,
                    sender: Actor::new(&row.get::<_, String>(2)?, &row.get::<_, String>(3)?),
                    content: row.get(4)?,
                    parent_id: row.get(5)?,
                    mtype: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// 保留策略清扫（§4.2.2）：删除 `retention_days` 天前的频道消息，
    /// 返回删除行数；`retention_days <= 0` = 永久保留，直接返回 0。
    /// master 启动 + 每日备份 cron 顺带调用。
    pub fn sweep_channel_messages(&self, retention_days: i64) -> Result<u64, String> {
        if retention_days <= 0 {
            return Ok(0);
        }
        let cutoff = Self::now() - retention_days * 24 * 3600;
        let n = self
            .conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute("DELETE FROM channel_message WHERE created_at < ?1", params![cutoff])
            .map_err(|e| e.to_string())?;
        Ok(n as u64)
    }

    /// 登记资产索引（实体文件由调用方先落 `assets/<ref>`，库只记索引）。
    /// 同 ref 重登记 = 幂等 upsert（新内容覆盖索引；path 恒 = ref）。
    pub fn register_asset(&self, new: NewAsset) -> Result<BoardAsset, String> {
        let ref_name = new.ref_name;
        if ref_name.trim().is_empty() {
            return Err("asset ref must not be empty".to_string());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = Self::now();
        conn.execute(
            "INSERT INTO asset (ref, origin_issue, sha256, size, path, created_at)
             VALUES (?1, ?2, ?3, ?4, ?1, ?5)
             ON CONFLICT(ref) DO UPDATE SET
                 origin_issue = excluded.origin_issue,
                 sha256 = excluded.sha256,
                 size = excluded.size",
            params![ref_name, new.origin_issue, new.sha256, new.size, now],
        )
        .map_err(|e| e.to_string())?;
        let id: i64 = conn
            .query_row("SELECT id FROM asset WHERE ref = ?1", params![ref_name], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        Ok(BoardAsset {
            id,
            ref_name: ref_name.clone(),
            origin_issue: new.origin_issue,
            sha256: new.sha256,
            size: new.size,
            path: ref_name,
            created_at: now,
        })
    }

    /// 按引用查资产（M3 下载端点前的 token 校验数据源）。
    pub fn lookup_asset(&self, ref_name: &str) -> Result<Option<BoardAsset>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT id, ref, origin_issue, sha256, size, path, created_at
             FROM asset WHERE ref = ?1",
            params![ref_name],
            |row| {
                Ok(BoardAsset {
                    id: row.get(0)?,
                    ref_name: row.get(1)?,
                    origin_issue: row.get(2)?,
                    sha256: row.get(3)?,
                    size: row.get(4)?,
                    path: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    /// 某个 issue 名下的资产列表。
    pub fn assets_for_issue(&self, issue_id: i64) -> Result<Vec<BoardAsset>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, ref, origin_issue, sha256, size, path, created_at
                 FROM asset WHERE origin_issue = ?1 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![issue_id], row_to_asset)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// issue 删除时解绑其名下资产：删 `origin_issue` 命中的行，返回因此
    /// **彻底失去引用**的 ref 列表（引用计数语义——多 issue 引用同一 ref
    /// 时最后一个解绑才删盘，§4.2.2）。调用方负责 unlink 返回的实体文件。
    pub fn unbind_assets_for_issue(&self, issue_id: i64) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let released: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT DISTINCT ref FROM asset WHERE origin_issue = ?1")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(params![issue_id], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
        };
        conn.execute("DELETE FROM asset WHERE origin_issue = ?1", params![issue_id])
            .map_err(|e| e.to_string())?;
        let mut fully_released = Vec::new();
        for ref_name in released {
            let remaining: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM asset WHERE ref = ?1",
                    params![ref_name],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if remaining == 0 {
                fully_released.push(ref_name);
            }
        }
        Ok(fully_released)
    }

    // -------------------------------------------------------------------
    // 团队经验 team_memory（swarm M4.5 §6.5）
    // -------------------------------------------------------------------

    /// 新增经验条目（验收蒸馏唯一写闸的第二道去噪闸）：scope 归一化后与
    /// 现存**未废弃**条目做「同 scope + 内容重复」判定——重复并进旧条目
    /// （use_count+1，旧条目身份稳定，注入端引用不漂移），否则插入新行。
    /// 返回（条目 id, 是否并入旧条目）。空 scope/content 的壳条目调用方
    /// 侧拒绝（评审蒸馏纪律），此处再兜底一次。
    pub fn add_team_memory(&self, new: NewTeamMemory) -> Result<(i64, bool), String> {
        let scope = new.scope.trim().to_lowercase();
        let content = new.content.trim().to_string();
        if scope.is_empty() || content.is_empty() {
            return Err("team memory scope/content must not be empty".to_string());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        // 同 scope 且内容归一化后相同（或互为包含——同义改写从宽并入）→
        // 重复。deprecated 条目不参与合并（修正语义：新条目可能就是取代
        // 旧经验的）。
        let dup: Option<i64> = conn
            .query_row(
                "SELECT id FROM team_memory
                 WHERE deprecated = 0 AND scope = ?1
                   AND (REPLACE(content, ' ', '') = ?2
                        OR instr(REPLACE(content, ' ', ''), ?2) > 0
                        OR instr(?2, REPLACE(content, ' ', '')) > 0)
                 ORDER BY id LIMIT 1",
                params![scope, content.replace(' ', "")],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(id) = dup {
            conn.execute(
                "UPDATE team_memory SET use_count = use_count + 1 WHERE id = ?1",
                params![id],
            )
            .map_err(|e| e.to_string())?;
            return Ok((id, true));
        }
        conn.execute(
            "INSERT INTO team_memory
                 (category, scope, content, source, author, use_count, deprecated, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, ?6)",
            params![
                new.category.trim(),
                scope,
                content,
                new.source.trim(),
                new.author.trim(),
                Self::now()
            ],
        )
        .map_err(|e| e.to_string())?;
        let id = conn.last_insert_rowid();
        Ok((id, false))
    }

    /// 经验条目列表（`include_deprecated=false` 时滤掉软删行；scope 过滤
    /// 精确匹配小写归一化）。排序：use_count 降序 → 新者优先（注入 top-N
    /// 与管理列表共用同一顺序）。
    pub fn list_team_memory(
        &self,
        scope: Option<&str>,
        include_deprecated: bool,
    ) -> Result<Vec<TeamMemoryEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut sql = String::from(
            "SELECT id, category, scope, content, source, author, use_count, deprecated, created_at
             FROM team_memory WHERE 1=1",
        );
        if let Some(s) = scope {
            sql.push_str(&format!(" AND scope = '{}'", s.trim().to_lowercase().replace('\'', "''")));
        }
        if !include_deprecated {
            sql.push_str(" AND deprecated = 0");
        }
        sql.push_str(" ORDER BY use_count DESC, id DESC");
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], row_to_team_memory)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// 关键词检索（scope/content/source LIKE，大小写不敏感；管理 WSAPI 用）。
    pub fn search_team_memory(&self, query: &str) -> Result<Vec<TeamMemoryEntry>, String> {
        let needle = format!("%{}%", query.trim().to_lowercase().replace('%', ""));
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, category, scope, content, source, author, use_count, deprecated, created_at
                 FROM team_memory
                 WHERE lower(scope) LIKE ?1 OR lower(content) LIKE ?1 OR lower(source) LIKE ?1
                 ORDER BY use_count DESC, id DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![needle], row_to_team_memory)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// 注入命中计数（派发注入 top-N 渲染成功后调用；衰减依据）。
    pub fn mark_team_memory_used(&self, ids: &[i64]) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        for id in ids {
            conn.execute(
                "UPDATE team_memory SET use_count = use_count + 1 WHERE id = ?1",
                params![id],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// 软删/恢复 deprecated 标记（§6.5.4 修正语义：旧经验与实际冲突 →
    /// 标记不覆盖；恢复 = 误标回滚）。
    pub fn set_team_memory_deprecated(&self, id: i64, deprecated: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n = conn
            .execute(
                "UPDATE team_memory SET deprecated = ?1 WHERE id = ?2",
                params![deprecated as i64, id],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!("team memory id={id} 不存在"));
        }
        Ok(())
    }

    /// 彻底删除条目（管理 WSAPI remove）。
    pub fn remove_team_memory(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n = conn
            .execute("DELETE FROM team_memory WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!("team memory id={id} 不存在"));
        }
        Ok(())
    }
}

/// 频道名归一化：trim + 缺 `#` 前缀自动补 + 非空校验。
fn normalize_channel_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("channel name must not be empty".to_string());
    }
    Ok(if trimmed.starts_with('#') {
        trimmed.to_string()
    } else {
        format!("#{trimmed}")
    })
}

// ---------------------------------------------------------------------------
// 行映射与内部 helper
// ---------------------------------------------------------------------------

fn display_assignee(issue: &Issue) -> String {
    match (&issue.assignee, &issue.assignee_id) {
        (Some(at), Some(aid)) => format!("{at}/{aid}"),
        _ => "（无）".to_string(),
    }
}

/// patch 应用：`Some(v)` 且 `v != old` → 记变更并返回新值；其余返回旧值。
/// （v == old 时也返回旧值，只是不记变更——幂等更新不算 diff。）
fn apply_patch<T: PartialEq + Clone>(
    changes: &mut Vec<String>,
    name: &str,
    old: &T,
    new: &Option<T>,
) -> T {
    match new {
        Some(v) if v != old => {
            changes.push(name.to_string());
            v.clone()
        }
        _ => old.clone(),
    }
}

/// 可空列（project_id/due_date/acceptance_criteria/parent_issue_id）的 patch：
/// patch `None` = 不动；`Some(v)` = 设为 v（v 为具体值；本 patch 形状不支持
/// 置 NULL——清空走建单或后续专用接口）。
fn apply_set_opt<T: PartialEq + Clone>(
    changes: &mut Vec<String>,
    name: &str,
    old: &Option<T>,
    new: &Option<T>,
) -> Option<T> {
    match new {
        Some(v) if old.as_ref() != Some(v) => {
            changes.push(name.to_string());
            Some(v.clone())
        }
        _ => old.clone(),
    }
}

/// 发频道消息的内核（不加锁）：空白内容拒绝 + 空 mtype 归一为 text +
/// 插入。[`BoardStore::append_channel_message`] 与 `post_discussion_envelope`
/// 共用——单一真相源。
fn append_channel_message_on(
    conn: &Connection,
    new: &NewChannelMessage,
) -> Result<ChannelMessage, String> {
    if new.content.trim().is_empty() {
        return Err("channel message content must not be empty".to_string());
    }
    let mtype = if new.mtype.trim().is_empty() {
        channel_message_type::TEXT.to_string()
    } else {
        new.mtype.clone()
    };
    let now = BoardStore::now();
    conn.execute(
        "INSERT INTO channel_message
             (channel_id, sender_type, sender_id, content, parent_id, mtype, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            new.channel_id,
            new.sender.kind,
            new.sender.id,
            new.content,
            new.parent_id,
            mtype,
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(ChannelMessage {
        id: conn.last_insert_rowid(),
        channel_id: new.channel_id,
        sender: new.sender.clone(),
        content: new.content.clone(),
        parent_id: new.parent_id,
        mtype,
        created_at: now,
    })
}

/// 落一条 issue 评论的完整副作用（不加锁内核）：FK 校验 + 评论行 +
/// 订阅 + activity + 站内通知（订阅者 ∪ 指派 − 作者；@提及优先）。
/// [`BoardStore::add_comment`] 与 `post_discussion_envelope` 共用——
/// 单一真相源，后者靠外层持同一把锁保证原子。
fn add_comment_on(conn: &Connection, new: &NewComment) -> Result<Comment, String> {
    // FK 校验 + 顺带取通知标题需要的编号/标题/指派（一次查询）。
    let (number, title, at, aid): (String, String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT number, title, assignee_type, assignee_id FROM issue WHERE id = ?1",
            params![new.issue_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("issue {} not found", new.issue_id))?;
    let issue_title = format!("{number} {title}");
    let assignee = at.zip(aid).map(|(k, i)| Actor::new(&k, &i));

    let now = BoardStore::now();
    let id = insert_comment(
        conn,
        new.issue_id,
        &new.author,
        &new.content,
        new.parent_id,
        new.ctype,
        now,
    )?;
    insert_subscriber(conn, new.issue_id, &new.author, "commented")?;
    insert_activity(conn, new.issue_id, &new.author, "commented", None, now)?;

    // 站内通知（W2 P3）：收件人 = 订阅者 ∪ 指派 − 作者；@提及优先于
    // 普通评论通知（同一人只收一条）。
    if new.ctype == CommentType::Comment {
        let mut recipients = self_subscribers(conn, new.issue_id)?;
        if let Some(a) = &assignee
            && !recipients.iter().any(|r| r == a)
        {
            recipients.push(a.clone());
        }
        let mentioned = extract_mentions(&new.content, &recipients);
        for m in &mentioned {
            if *m != new.author {
                insert_notification(
                    conn,
                    &NewNotification {
                        recipient: m.clone(),
                        kind: notification_kind::MENTIONED.to_string(),
                        title: issue_title.clone(),
                        content: new.content.clone(),
                        issue_id: Some(new.issue_id),
                    },
                    now,
                )?;
            }
        }
        for r in &recipients {
            if *r == new.author || mentioned.iter().any(|m| m == r) {
                continue;
            }
            insert_notification(
                conn,
                &NewNotification {
                    recipient: r.clone(),
                    kind: notification_kind::COMMENTED.to_string(),
                    title: issue_title.clone(),
                    content: new.content.clone(),
                    issue_id: Some(new.issue_id),
                },
                now,
            )?;
        }
    }

    Ok(Comment {
        id,
        issue_id: new.issue_id,
        author: new.author.clone(),
        content: new.content.clone(),
        parent_id: new.parent_id,
        ctype: new.ctype,
        created_at: now,
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_comment(
    conn: &Connection,
    issue_id: i64,
    author: &Actor,
    content: &str,
    parent_id: Option<i64>,
    ctype: CommentType,
    now: i64,
) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO comment (issue_id, author_type, author_id, content, parent_id, ctype, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![issue_id, author.kind, author.id, content, parent_id, ctype.as_str(), now],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

fn insert_activity(
    conn: &Connection,
    issue_id: i64,
    actor: &Actor,
    action: &str,
    details: Option<&str>,
    now: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO activity_log (issue_id, actor_type, actor_id, action, details, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![issue_id, actor.kind, actor.id, action, details, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn insert_subscriber(
    conn: &Connection,
    issue_id: i64,
    who: &Actor,
    reason: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO issue_subscriber (issue_id, subscriber_type, subscriber_id, reason)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (issue_id, subscriber_type, subscriber_id)
         DO UPDATE SET reason = excluded.reason",
        params![issue_id, who.kind, who.id, reason],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn insert_notification(conn: &Connection, n: &NewNotification, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO notification (recipient_type, recipient_id, kind, title, content, issue_id, read, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
        params![n.recipient.kind, n.recipient.id, n.kind, n.title, n.content, n.issue_id, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// issue 的订阅者列表（通知收件人候选；轻量查询，无 reason）。
fn self_subscribers(conn: &Connection, issue_id: i64) -> Result<Vec<Actor>, String> {
    let mut stmt = conn
        .prepare("SELECT subscriber_type, subscriber_id FROM issue_subscriber WHERE issue_id = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![issue_id], |r| {
            Ok(Actor::new(&r.get::<_, String>(0)?, &r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 从评论内容提取 @提及：`@<id>` token，与候选 Actor 的 id 精确匹配（kind
/// 不区分——看板里节点 id 即身份）。头字符剥到 `@` 为止（容忍 `(@id`、
/// 中日韩文紧邻）；尾部只剥标点、保留 `-`/`_`（节点 id 常含连字符，如
/// `@node-b`）。未命中候选的 @token 静默忽略（自由文本不报错）；去重保序。
fn extract_mentions(content: &str, candidates: &[Actor]) -> Vec<Actor> {
    let mut hits: Vec<Actor> = Vec::new();
    for token in content.split_whitespace() {
        let trimmed = token
            .trim_start_matches(|c: char| c != '@')
            .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
        let Some(mentioned) = trimmed.strip_prefix('@') else {
            continue;
        };
        if mentioned.is_empty() {
            continue;
        }
        if let Some(a) = candidates.iter().find(|a| a.id == mentioned)
            && !hits.iter().any(|h: &Actor| h.id == a.id)
        {
            hits.push(a.clone());
        }
    }
    hits
}

fn row_to_dispatch(row: &rusqlite::Row<'_>) -> rusqlite::Result<DispatchRecord> {
    Ok(DispatchRecord {
        task_id: row.get(0)?,
        issue_id: row.get(1)?,
        worker_id: row.get(2)?,
        state: row.get(3)?,
        dispatched_at: row.get(4)?,
        completed_at: row.get(5)?,
    })
}

fn row_to_asset(row: &rusqlite::Row<'_>) -> rusqlite::Result<BoardAsset> {
    Ok(BoardAsset {
        id: row.get(0)?,
        ref_name: row.get(1)?,
        origin_issue: row.get(2)?,
        sha256: row.get(3)?,
        size: row.get(4)?,
        path: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// team_memory 行映射（M4.5 §6.5.1；SELECT 列序必须与本函数一一对应）。
fn row_to_team_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<TeamMemoryEntry> {
    let deprecated: i64 = row.get(7)?;
    Ok(TeamMemoryEntry {
        id: row.get(0)?,
        category: row.get(1)?,
        scope: row.get(2)?,
        content: row.get(3)?,
        source: row.get(4)?,
        author: row.get(5)?,
        use_count: row.get(6)?,
        deprecated: deprecated != 0,
        created_at: row.get(8)?,
    })
}

fn row_to_autopilot(row: &rusqlite::Row<'_>) -> rusqlite::Result<Autopilot> {
    let enabled: i64 = row.get("enabled")?;
    Ok(Autopilot {
        id: row.get("id")?,
        name: row.get("name")?,
        cron: row.get("cron")?,
        title: row.get("title")?,
        description: row.get("description")?,
        priority: row.get("priority")?,
        project_id: row.get("project_id")?,
        target: row.get("target")?,
        enabled: enabled != 0,
        cron_job_id: row.get("cron_job_id")?,
        last_run_at: row.get("last_run_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_notification(row: &rusqlite::Row<'_>) -> rusqlite::Result<Notification> {
    let read: i64 = row.get("read")?;
    Ok(Notification {
        id: row.get("id")?,
        recipient: Actor::new(
            &row.get::<_, String>("recipient_type")?,
            &row.get::<_, String>("recipient_id")?,
        ),
        kind: row.get("kind")?,
        title: row.get("title")?,
        content: row.get("content")?,
        issue_id: row.get("issue_id")?,
        read: read != 0,
        created_at: row.get("created_at")?,
    })
}

fn row_to_issue(row: &rusqlite::Row<'_>) -> rusqlite::Result<Issue> {
    let status_s: String = row.get("status")?;
    let status = IssueStatus::from_str(&status_s).unwrap_or(IssueStatus::Backlog);
    let assignee_type: Option<String> = row.get("assignee_type")?;
    let origin_type: Option<String> = row.get("origin_type")?;
    let origin_id: Option<String> = row.get("origin_id")?;
    // Swarm M1：required_tags JSON TEXT → Vec（坏 JSON/NULL 宽容为空）。
    let required_tags_json: Option<String> = row.get("required_tags")?;
    let required_tags = required_tags_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    Ok(Issue {
        id: row.get("id")?,
        number: row.get("number")?,
        title: row.get("title")?,
        description: row.get("description")?,
        status,
        priority: row.get("priority")?,
        assignee: assignee_type.as_deref().and_then(AssignmentType::from_str),
        assignee_id: row.get("assignee_id")?,
        creator: Actor::new(
            &row.get::<_, String>("creator_type")?,
            &row.get::<_, String>("creator_id")?,
        ),
        parent_issue_id: row.get("parent_issue_id")?,
        project_id: row.get("project_id")?,
        due_date: row.get("due_date")?,
        position: row.get("position")?,
        acceptance_criteria: row.get("acceptance_criteria")?,
        origin: match (origin_type, origin_id) {
            (Some(t), Some(i)) => Some(crate::models::TaskOrigin {
                origin_type: t,
                origin_id: i,
            }),
            _ => None,
        },
        required_role: row
            .get::<_, Option<String>>("required_role")?
            .filter(|s| !s.trim().is_empty()),
        required_tags,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_comment(row: &rusqlite::Row<'_>) -> rusqlite::Result<Comment> {
    Ok(Comment {
        id: row.get("id")?,
        issue_id: row.get("issue_id")?,
        author: Actor::new(
            &row.get::<_, String>("author_type")?,
            &row.get::<_, String>("author_id")?,
        ),
        content: row.get("content")?,
        parent_id: row.get("parent_id")?,
        ctype: CommentType::from_str(&row.get::<_, String>("ctype")?)
            .unwrap_or(CommentType::Comment),
        created_at: row.get("created_at")?,
    })
}

fn row_to_activity(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActivityLog> {
    Ok(ActivityLog {
        id: row.get("id")?,
        issue_id: row.get("issue_id")?,
        actor: Actor::new(
            &row.get::<_, String>("actor_type")?,
            &row.get::<_, String>("actor_id")?,
        ),
        action: row.get("action")?,
        details: row.get("details")?,
        created_at: row.get("created_at")?,
    })
}

fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    let lead_type: Option<String> = row.get("lead_type")?;
    let lead_id: Option<String> = row.get("lead_id")?;
    Ok(Project {
        id: row.get("id")?,
        name: row.get("name")?,
        description: row.get("description")?,
        status: row.get("status")?,
        priority: row.get("priority")?,
        lead: lead_type.zip(lead_id).map(|(k, i)| Actor::new(&k, &i)),
        icon: row.get("icon")?,
        created_at: row.get("created_at")?,
    })
}

#[cfg(test)]
mod tests;
