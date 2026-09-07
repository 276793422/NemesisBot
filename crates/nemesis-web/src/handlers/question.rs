//! F7（devtool-upgrade 阶段 5）— `question.respond` / `question.pending`。
//!
//! Dashboard 提问卡的 WSAPI 入口：把用户作答送回等待中的
//! `WebQuestionBroker::ask`（question 工具的阻塞点）。Responder 经
//! `ctx.state.agent_loop` 的 question_responder 槽触达（gateway 装配时
//! `set_question_responder` 注入）——未装配（headless / 旧装配）时诚实报
//! 「未装配」。载荷校验（空选择/非候选/单选多项）在 broker 侧，这里只做
//! 传输形态校验（question_id 存在、selected 是字符串数组）。

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct QuestionHandler;

#[async_trait::async_trait]
impl ModuleHandler for QuestionHandler {
    fn module_name(&self) -> &str {
        "question"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["respond", "pending"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // 槽读取窗口尽量窄：clone 出 responder Arc 后立即放锁，respond 是
        // 无 await 的同步调用（mpsc send），不持 parking_lot 锁跨 await。
        let responder = {
            let guard = ctx.state.agent_loop.read();
            match guard.as_ref() {
                None => return Err("agent loop not running".to_string()),
                Some(loop_ref) => loop_ref.question_responder(),
            }
        };
        let responder = responder
            .ok_or_else(|| "question broker not wired (no interactive session)".to_string())?;
        match cmd {
            "respond" => {
                let data = data.ok_or("missing data")?;
                let question_id = data
                    .get("question_id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing question_id")?;
                let selected = data
                    .get("selected")
                    .and_then(|v| v.as_array())
                    .ok_or("missing selected (expected array of strings)")?
                    .iter()
                    .map(|x| x.as_str().map(String::from))
                    .collect::<Option<Vec<String>>>()
                    .ok_or("selected must be an array of strings")?;
                let delivered = responder.respond(question_id, selected)?;
                tracing::info!(
                    "[Web/WSAPI] question.respond id={} -> delivered={}",
                    question_id,
                    delivered
                );
                Ok(Some(serde_json::json!({
                    "question_id": question_id,
                    "delivered": delivered,
                })))
            }
            "pending" => Ok(Some(serde_json::json!({
                "pending": responder.pending(),
            }))),
            _ => Err(format!("unknown command: question.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
