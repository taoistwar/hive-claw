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
    ProgressParams, WebuiTurnCoordinator, build_bus_progress_callback, clean_generated_title,
    mark_webui_session, maybe_generate_webui_title, maybe_generate_webui_title_after_turn,
    publish_turn_run_status, websocket_turn_wall_started_at,
};
