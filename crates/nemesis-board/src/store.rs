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
    ActivityLog, Attachment, Autopilot, AutopilotPatch, Comment, CommentType, DispatchRecord,
    Issue, IssueFilter, IssuePatch, IssueStatus, NewAutopilot, NewComment, NewIssue,
    NewNotification, Notification, Project, ProjectPatch, Subscriber, dispatch_state,
    notification_kind,
};

/// Thread-safe SQLite board store.
pub struct BoardStore {
    conn: Mutex<Connection>,
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
        })
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
        tx.execute(
            "INSERT INTO issue (number, title, description, status, priority,
                assignee_type, assignee_id, creator_type, creator_id,
                parent_issue_id, project_id, due_date, position,
                acceptance_criteria, origin_type, origin_id, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?17)",
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

        let now = Self::now();
        let id = insert_comment(
            &conn,
            new.issue_id,
            &new.author,
            &new.content,
            new.parent_id,
            new.ctype,
            now,
        )?;
        insert_subscriber(&conn, new.issue_id, &new.author, "commented")?;
        insert_activity(&conn, new.issue_id, &new.author, "commented", None, now)?;

        // 站内通知（W2 P3）：收件人 = 订阅者 ∪ 指派 − 作者；@提及优先于
        // 普通评论通知（同一人只收一条）。
        if new.ctype == CommentType::Comment {
            let mut recipients = self_subscribers(&conn, new.issue_id)?;
            if let Some(a) = &assignee
                && !recipients.iter().any(|r| r == a)
            {
                recipients.push(a.clone());
            }
            let mentioned = extract_mentions(&new.content, &recipients);
            for m in &mentioned {
                if *m != new.author {
                    insert_notification(
                        &conn,
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
                    &conn,
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
            author: new.author,
            content: new.content,
            parent_id: new.parent_id,
            ctype: new.ctype,
            created_at: now,
        })
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
