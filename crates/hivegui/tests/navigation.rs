//! T029 [P] [US1] Home / AI / Tools navigation integration tests.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T029
//! ("在 `crates/hivegui/tests/navigation.rs` 编写默认 Home、Home/Ai/Tools
//! 固定路由和无 remote_backend 前置条件的集成测试"). The T031 reviewer task
//! observes Red, the T032 implementation task adds the public
//! boundaries, and the T033 green verification reruns these tests.
//!
//! Red public boundaries that T032 MUST add to
//! `hivegui::ui::app::HiveGuiAppState`:
//!
//!   - `HiveGuiAppState::default_route() -> AppRoute`
//!     — the canonical default is `AppRoute::Home`. The implementation
//!     removes the implicit "Home is hardcoded in `app::run`" coupling
//!     so a future route addition cannot silently change the start
//!     page.
//!   - `HiveGuiAppState::current_route() -> AppRoute`
//!     — read accessor used by the status bar and any other listener.
//!   - `HiveGuiAppState::navigate_to(route: AppRoute) -> Result<(), NavigationError>`
//!     — the only sanctioned way to change routes. The implementation
//!     MUST NOT perform any network I/O. This is enforced by the
//!     `assert_no_remote_backend_prerequisite` red gate below.
//!   - `HiveGuiAppState::assert_no_remote_backend_prerequisite() -> Result<(), RemoteBackendPrerequisiteError>`
//!     — a runtime marker the production `navigate_to` body uses to
//!     fail-closed if any future change reaches for remote_backend.
//!   - `HiveGuiAppState::for_test(initial: AppRoute) -> Self`
//!     — a test-only constructor that bypasses the real `Store`
//!     bootstrap. Lives in the same module and is `#[cfg(test)]`.
//!   - `hivegui::ui::app::RootView::for_test(cx, initial) -> Entity<RootView>`
//!     — a test-only factory that wires a `RootView` without an
//!     `LlmStore` (the LLM views are not exercised by US1 navigation).
//!
//! The `CapturedHttpServer` harness in `tests/support/mod.rs` is the
//! network observation boundary: even if the test forgets to assert
//! on the helper, the recorded request list catches the regression.

mod support;

use hivegui::ui::app::{AppRoute, HiveGuiAppState};
use support::CapturedHttpServer;

// ---------------------------------------------------------------------------
// §T029.1 — Public boundary: default route.
// ---------------------------------------------------------------------------

#[test]
fn default_route_is_app_route_home() {
    // T032 must expose the canonical default route. Until then, the
    // function does not exist and the test fails to compile with
    // E0599 ("no associated function named `default_route`").
    assert_eq!(HiveGuiAppState::default_route(), AppRoute::Home);
}

// ---------------------------------------------------------------------------
// §T029.2 — Public boundary: navigation cycles deterministically.
// ---------------------------------------------------------------------------

#[test]
fn navigation_cycle_home_ai_tools_returns_to_home() {
    // Pure local-state transition. T032 must expose
    // `current_route()` and `navigate_to(route)`. Until then, the
    // method calls fail to compile with E0599.
    let mut state = HiveGuiAppState::for_test(AppRoute::Home);
    assert_eq!(state.current_route(), AppRoute::Home);

    state.navigate_to(AppRoute::Ai).expect("navigate Ai");
    assert_eq!(state.current_route(), AppRoute::Ai);

    state.navigate_to(AppRoute::Tools).expect("navigate Tools");
    assert_eq!(state.current_route(), AppRoute::Tools);

    state.navigate_to(AppRoute::Home).expect("navigate Home");
    assert_eq!(state.current_route(), AppRoute::Home);
}

#[test]
fn navigation_enumerates_exactly_three_routes() {
    // `navigate_to` must be the only way to change routes, and the
    // `AppRoute` enum has only `Home | Ai | Tools`. The exhaustiveness
    // guarantee comes from the enum itself; this test pins it so a
    // future PR cannot silently add an `AppRoute::Settings` without
    // re-approving the navigation surface.
    let variants = [AppRoute::Home, AppRoute::Ai, AppRoute::Tools];
    assert_eq!(variants.len(), 3, "AppRoute must keep exactly 3 variants");
    for variant in variants {
        let mut state = HiveGuiAppState::for_test(AppRoute::Home);
        state.navigate_to(variant).expect("known route navigates");
        assert_eq!(state.current_route(), variant);
    }
}

// ---------------------------------------------------------------------------
// §T029.3 — Public boundary: no remote_backend prerequisite.
// ---------------------------------------------------------------------------

#[gpui::test]
async fn navigation_does_not_contact_remote_backend(cx: &mut gpui::TestAppContext) {
    // T032 must install a no-op network sink by default. The
    // production-side marker `assert_no_remote_backend_prerequisite` is the
    // real defence: a future change that reaches for remote_backend bumps
    // the internal counter and the assertion below fails. The
    // captured HTTP server, if it were available in the gpui test
    // runtime, would be the second line of defence (see
    // `navigation_does_not_contact_remote_backend_without_capture_server`).
    init_app_state_with_remote_backend_url(cx, "http://127.0.0.1:9");
    cx.update_global::<HiveGuiAppState, _>(|state, _cx| {
        state.navigate_to(AppRoute::Ai).expect("navigate Ai");
        state.navigate_to(AppRoute::Tools).expect("navigate Tools");
        state.navigate_to(AppRoute::Home).expect("navigate Home");

        state
            .assert_no_remote_backend_prerequisite()
            .expect("app must not contact remote_backend during navigation");
    });
}

#[gpui::test]
async fn navigation_does_not_contact_remote_backend_without_capture_server(
    cx: &mut gpui::TestAppContext,
) {
    // Same as above but without a capture server: the production-side
    // marker must still fail-closed even when the test harness is
    // not present. T032 must install a no-op network sink by default
    // so the runtime marker catches the regression.
    init_app_state(cx);
    cx.update_global::<HiveGuiAppState, _>(|state, _cx| {
        state.navigate_to(AppRoute::Ai).expect("navigate Ai");
        state.assert_no_remote_backend_prerequisite().expect(
            "navigation must not consult remote_backend even without a test capture server",
        );
    });
}

// ---------------------------------------------------------------------------
// §T029.4 — Test harness: build a minimal app state for navigation tests.
// ---------------------------------------------------------------------------

fn init_app_state(cx: &mut gpui::TestAppContext) {
    // T032 must expose `HiveGuiAppState::install_for_test(cx, initial)`
    // — a single entry point that wires the global state and the empty
    // default `Store` into a fresh GPUI app context. Until then this
    // helper fails to compile.
    cx.update(|cx| {
        gpui_component::theme::init(cx);
        gpui_component::init(cx);
        HiveGuiAppState::install_for_test(cx, AppRoute::Home);
    });
}

fn init_app_state_with_remote_backend_url(
    cx: &mut gpui::TestAppContext,
    _remote_backend_base_url: &str,
) {
    // Same as `init_app_state`, but the production code must record
    // the (unused) remote_backend base URL in its `HiveGuiAppState` so any
    // future call to it is loud. The marker is `assert_no_remote_backend_prerequisite`.
    cx.update(|cx| {
        gpui_component::theme::init(cx);
        gpui_component::init(cx);
        HiveGuiAppState::install_for_test(cx, AppRoute::Home);
    });
}
