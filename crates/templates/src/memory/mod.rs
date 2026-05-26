//! Memory-related templates for nanobot.
//!
//! Rust port of `nanobot.templates.memory`. Contains the long-term memory
//! template that persists important information across sessions.

use std::collections::HashMap;

/// Default content for the MEMORY.md template file.
pub const MEMORY_TEMPLATE: &str = include_str!("../../templates/memory/MEMORY.md");

/// Render the memory template with optional context variables.
///
/// Currently this returns the raw template as-is, but can be extended
/// to substitute variables if needed in the future.
pub fn render_memory_template(_context: &HashMap<String, String>) -> String {
    MEMORY_TEMPLATE.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_template_is_available() {
        let ctx = HashMap::new();
        let rendered = render_memory_template(&ctx);
        assert!(rendered.contains("# Long-term Memory"));
        assert!(rendered.contains("User Information"));
        assert!(rendered.contains("Preferences"));
        assert!(rendered.contains("Project Context"));
        assert!(rendered.contains("Important Notes"));
    }
}
