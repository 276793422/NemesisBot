//! Agent routing and session key management.

pub mod agent_id;
pub mod route;
pub mod session_key;

pub use agent_id::{normalize_agent_id, normalize_account_id, DEFAULT_AGENT_ID, DEFAULT_ACCOUNT_ID};
pub use route::{RouteResolver, RouteInput, ResolvedRoute, RouteConfig, AgentBinding, AgentDef};
pub use session_key::{
    build_agent_session_key, build_agent_main_session_key,
    build_agent_peer_session_key, parse_agent_session_key,
    is_subagent_session_key,
    DMScope, RoutePeer, SessionKeyParams,
};
