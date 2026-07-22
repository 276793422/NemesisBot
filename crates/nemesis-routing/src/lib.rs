//! Agent routing and session key management.

pub mod agent_id;
pub mod route;
pub mod session_key;

pub use agent_id::{
    DEFAULT_ACCOUNT_ID, DEFAULT_AGENT_ID, normalize_account_id, normalize_agent_id,
};
pub use route::{AgentBinding, AgentDef, ResolvedRoute, RouteConfig, RouteInput, RouteResolver};
pub use session_key::{
    DMScope, RoutePeer, SessionKeyParams, build_agent_main_session_key,
    build_agent_peer_session_key, build_agent_session_key, is_subagent_session_key,
    parse_agent_session_key,
};
