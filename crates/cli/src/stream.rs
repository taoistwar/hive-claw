/// Streaming renderer for CLI output.
///
/// Uses Rich Live with `transient=true` for in-place markdown updates during
/// streaming. After the live display stops, a final clean render is printed
/// so the content persists on screen. `transient=true` ensures the live
/// area is erased before `stop()` returns, avoiding the duplication bug
/// that plagued earlier approaches.
use std::fmt::Write as _;
use std::io::Write as _;

use tokio::sync::Mutex;

/// Erase a transient status line before printing persistent output.
fn clear_current_line() {
    use std::io::{IsTerminal, Write as _};

    let stdout = std::io::stdout();
    if !stdout.is_terminal() {
        return;
    }
    let mut handle = stdout.lock();
    let _ = write!(handle, "\r\x1b[2K");
    let _ = handle.flush();
}

/// Spinner that shows "<bot_name> is thinking..." with pause support.
///
/// NOTE: Full Rich spinner support requires the `crossterm` / `console`
/// ecosystem. This implementation uses TODO placeholders for the actual
/// spinner rendering.
pub struct ThinkingSpinner {
    bot_name: String,
    active: bool,
}

impl ThinkingSpinner {
    pub fn new(bot_name: &str) -> Self {
        Self {
            bot_name: bot_name.to_string(),
            active: false,
        }
    }

    /// Start the spinner.
    pub fn start(&mut self) {
        // TODO: Integrate with Rich-equivalent spinner via crossterm/console
        self.active = true;
    }

    /// Stop the spinner and clear the line.
    pub fn stop(&mut self) {
        self.active = false;
        clear_current_line();
    }

    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// Streaming renderer with Rich Live for in-place updates.
///
/// During streaming: updates content in-place via Rich Live.
/// On end: stops Live (transient=true erases it), then prints final render.
///
/// Flow per round:
///   spinner -> first delta -> header + Live updates ->
///   on_end -> stop Live + final render
pub struct StreamRenderer {
    md: bool,
    show_spinner: bool,
    bot_name: String,
    bot_icon: String,
    buf: String,
    pub streamed: bool,
    spinner: Option<ThinkingSpinner>,
    header_printed: bool,
    live_active: bool,
}

impl StreamRenderer {
    pub fn new(render_markdown: bool, show_spinner: bool, bot_name: &str, bot_icon: &str) -> Self {
        let mut renderer = Self {
            md: render_markdown,
            show_spinner,
            buf: String::new(),
            streamed: false,
            bot_name: bot_name.to_string(),
            bot_icon: bot_icon.to_string(),
            spinner: None,
            header_printed: false,
            live_active: false,
        };
        renderer.start_spinner();
        renderer
    }

    fn renderable(&self) -> String {
        if self.md && !self.buf.is_empty() {
            // TODO: Parse and render markdown (use `comrak` or similar)
            self.buf.clone()
        } else {
            self.buf.clone()
        }
    }

    fn render_str(&self) -> String {
        // TODO: Render through Rich-equivalent for proper formatting
        self.renderable()
    }

    fn start_spinner(&mut self) {
        if self.show_spinner {
            let mut spinner = ThinkingSpinner::new(&self.bot_name);
            spinner.start();
            self.spinner = Some(spinner);
        }
    }

    fn stop_spinner(&mut self) {
        if let Some(mut spinner) = self.spinner.take() {
            spinner.stop();
        }
    }

    pub fn header_printed(&self) -> bool {
        self.header_printed
    }

    /// Stop transient status and print the assistant header once.
    pub fn ensure_header(&mut self) {
        self.stop_spinner();
        if self.header_printed {
            return;
        }
        let header = if !self.bot_icon.is_empty() {
            format!("{} {}", self.bot_icon, self.bot_name)
        } else {
            self.bot_name.clone()
        };
        println!("\n{header}");
        self.header_printed = true;
    }

    /// Context manager: temporarily stop transient output for clean trace lines.
    ///
    /// Returns a guard that restores the live view when dropped.
    pub fn pause_spinner(&mut self) -> PauseGuard<'_> {
        let was_active = self.live_active;
        if self.live_active {
            // TODO: Stop Rich Live equivalent
            self.live_active = false;
        }
        PauseGuard {
            renderer: self,
            live_was_active: was_active,
        }
    }

    pub async fn on_delta(&mut self, delta: &str) {
        self.streamed = true;
        self.buf.push_str(delta);

        if !self.live_active {
            if self.buf.trim().is_empty() {
                return;
            }
            self.ensure_header();
            // TODO: Start Rich Live equivalent with transient=true
            self.live_active = true;
        } else {
            // TODO: Update Rich Live equivalent
        }
        // TODO: Refresh Rich Live equivalent
    }

    pub async fn on_end(&mut self, resuming: bool) {
        if self.live_active {
            // TODO: Double-refresh Rich Live equivalent
            // TODO: Stop Rich Live equivalent
            self.live_active = false;
        }
        self.stop_spinner();
        if !self.buf.trim().is_empty() {
            // Print final rendered content
            let out = self.render_str();
            print!("{out}");
            let _ = std::io::stdout().flush();
        }
        if resuming {
            self.buf.clear();
            self.start_spinner();
        }
    }

    /// Stop spinner before user input to avoid prompt_toolkit conflicts.
    pub fn stop_for_input(&mut self) {
        self.stop_spinner();
    }

    pub async fn close(&mut self) {
        if self.live_active {
            // TODO: Stop Rich Live equivalent
            self.live_active = false;
        }
        self.stop_spinner();
    }
}

/// Guard returned by [`StreamRenderer::pause_spinner`].
/// Restores the live view when dropped.
pub struct PauseGuard<'a> {
    renderer: &'a mut StreamRenderer,
    live_was_active: bool,
}

impl Drop for PauseGuard<'_> {
    fn drop(&mut self) {
        // If the live was active and no new deltas have started a fresh Live,
        // restore it. For simplicity, we only re-set the flag.
        // TODO: Re-start Rich Live equivalent if needed
        if self.live_was_active && !self.renderer.live_active {
            // The renderer's on_delta will recreate the Live when more deltas arrive.
        }
    }
}
