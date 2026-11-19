//! 提示词查看器一次性访问票据（2026-11-13 UX 收口）。
//!
//! 用户拍板：查看器页是**一次性页面**——链接打开一次后 10 秒内有效，
//! 之后链接不可访问（刷新/重放/转发全失效）；已打开的页面内容驻留内存
//! 可持续查看。纯前端倒计时防不了刷新重放，真正的「链接不可访问」必须
//! 服务端裁决，故采用票据流：
//!
//! ```text
//! ChatPanel 段按钮 → chat.prompt_ticket（发票，绑定 session_key+segment，
//!                   TTL 10s，全局单次消费）→ 查看器 URL 携 &ticket=
//! 查看器加载      → chat.prompt_segment + ticket → 消费（remove-if-valid）
//!                 → 二次取数（刷新）= 票据已消费/过期 → 诚实报错
//! ```
//!
//! 存放形态：模块级 `OnceLock`（同 [`crate::chat_event_log`] 的理由——
//! AppState 字面量散布 69 个测试文件不可动，加字段是全库爆破；模块级
//! 快照保有单一真相源性质且零爆破半径）。票据是短命 10s 单次凭证，内存态
//! 即可：重启即清（重启后旧链接本就该死），无持久化必要。
//!
//! 安全语义：票据不替代登录态——查看器仍需 localStorage 凭据过 WS 鉴权；
//! 票据只是把「URL 可无限重放」收窄成「URL 一次性」。绑定 (session_key,
//! segment) 二元组，错配即拒。

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 票据有效期（秒）= 查看器页倒计时口径（用户拍板 10 秒）。
pub const PROMPT_TICKET_TTL_SECS: u64 = 10;
/// 票据表容量上界（防泄漏；正常使用远达不到——每张票 10s 内自然过期）。
const MAX_TICKETS: usize = 512;

#[derive(Debug, Clone)]
struct TicketEntry {
    session_key: String,
    segment: String,
    expires_at: Instant,
}

fn store() -> &'static Mutex<HashMap<String, TicketEntry>> {
    static STORE: OnceLock<Mutex<HashMap<String, TicketEntry>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 发出的票据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PromptTicket {
    pub ticket: String,
    pub ttl_secs: u64,
}

/// 发票：绑定 (session_key, segment)，TTL [`PROMPT_TICKET_TTL_SECS`]。
///
/// `ticket` 由调用方生成（uuid），本模块不引入 RNG 依赖。插入时顺带清扫
/// 过期项（有界内存；清扫成本 O(表长)，表长受 MAX_TICKETS 钳制）。
pub fn issue(ticket: &str, session_key: &str, segment: &str, now: Instant) -> PromptTicket {
    let ttl = Duration::from_secs(PROMPT_TICKET_TTL_SECS);
    let mut map = store().lock();
    map.retain(|_, e| now < e.expires_at);
    if map.len() >= MAX_TICKETS {
        // 满表 = 异常风暴（正常 10s 内自然泄洪）；诚实拒绝新票，不静默挤兑。
        tracing::warn!("[prompt_ticket] ticket table full ({MAX_TICKETS}), rejecting issue");
    } else {
        map.insert(
            ticket.to_string(),
            TicketEntry {
                session_key: session_key.to_string(),
                segment: segment.to_string(),
                expires_at: now + ttl,
            },
        );
    }
    PromptTicket {
        ticket: ticket.to_string(),
        ttl_secs: PROMPT_TICKET_TTL_SECS,
    }
}

/// 消费（单次语义 = 取出即删）：票据不存在/已用/已过期/(会话,段) 错配
/// 均诚实报错，错误文案面向终端用户（查看器错误卡直显）。
pub fn consume(ticket: &str, session_key: &str, segment: &str, now: Instant) -> Result<(), String> {
    let entry = store().lock().remove(ticket);
    let Some(entry) = entry else {
        return Err(
            "链接已失效：本页为一次性访问页，链接打开一次后即作废。请回到 Dashboard，从构成条弹层的段按钮重新打开".to_string(),
        );
    };
    if now >= entry.expires_at {
        return Err(
            "链接已过期（10 秒时效）：请回到 Dashboard，从构成条弹层的段按钮重新打开".to_string(),
        );
    }
    if entry.session_key != session_key || entry.segment != segment {
        return Err("票据与会话/段不匹配：链接参数已被篡改或串用".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod prompt_ticket_tests;
