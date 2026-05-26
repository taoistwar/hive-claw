//! Session management module.

pub mod goal_state;
pub mod manager;
pub mod webui_turns;

pub use goal_state::{
    GOAL_STATE_KEY, discard_legacy_goal_state_key, goal_state_raw, goal_state_runtime_lines,
    goal_state_ws_blob, parse_goal_state, runner_wall_llm_timeout_s, sustained_goal_active,
};
pub use manager::{Session, SessionManager};
pub use webui_turns::{
    build_webui_goal_state, clean_generated_title, is_webui_session, mark_webui_session,
    maybe_generate_webui_title, record_turn_start, clear_turn_start, title_inputs,
    websocket_turn_wall_started_at,
};
