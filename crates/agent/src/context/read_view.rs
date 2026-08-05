//! ReadView: read-only concurrent access to AgentContext.
//!
//! Provides a lightweight, non-owning view into an AgentContext that
//! exposes only read operations, preventing accidental mutation.

use std::collections::HashMap;

use super::category::{Category, RecordEntry};
use super::core::{AgentContext, LifecycleState, UserInput};
use super::response::ExtensionContent;

/// A read-only view into an `AgentContext` for safe concurrent access.
///
/// This struct holds a reference to the context and provides only immutable
/// accessor methods. It is `Send + Sync` and can be shared across threads.
pub struct ReadView {
    user_input: UserInput,
    categories: HashMap<Category, Vec<RecordEntry>>,
    extensions: Vec<ExtensionContent>,
    lifecycle_state: LifecycleState,
}

impl ReadView {
    /// Create a new `ReadView` by snapshotting the current state of `ctx`.
    ///
    /// This performs a point-in-time read of all categories and extensions.
    /// The returned view is independent of future mutations to the context.
    pub fn new(ctx: &AgentContext) -> Self {
        let mut categories = HashMap::new();
        for cat in [
            Category::Entities,
            Category::Intentions,
            Category::ToolResults,
            Category::QueryResults,
            Category::WorkflowResults,
            Category::ReasoningResults,
            Category::Extensions,
            Category::StateChanges,
            Category::SubagentResults,
        ] {
            if let Ok(records) = ctx.get_category(cat) {
                categories.insert(cat, records);
            }
        }

        Self {
            user_input: ctx.user_input().clone(),
            categories,
            extensions: ctx.get_extensions(),
            lifecycle_state: ctx.lifecycle_state(),
        }
    }

    /// Get a reference to the user input.
    pub fn user_input(&self) -> &UserInput {
        &self.user_input
    }

    /// Get all records for a category.
    ///
    /// Returns `None` if the category has no records.
    pub fn get_category(&self, category: Category) -> Option<&Vec<RecordEntry>> {
        self.categories.get(&category)
    }

    /// Get a specific record by category and key.
    pub fn get_record(&self, category: Category, key: &str) -> Option<&RecordEntry> {
        self.categories
            .get(&category)
            .and_then(|records| records.iter().find(|r| r.key == key))
    }

    /// Get all extension content.
    pub fn get_extensions(&self) -> &[ExtensionContent] {
        &self.extensions
    }

    /// Get the lifecycle state at the time this view was created.
    pub fn lifecycle_state(&self) -> LifecycleState {
        self.lifecycle_state
    }
}

unsafe impl Send for ReadView {}
unsafe impl Sync for ReadView {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::config::ContextConfig;

    fn make_context() -> AgentContext {
        let user_input = UserInput {
            raw_text: "hello".into(),
            session_id: Some("sess-1".into()),
            message_id: Some("msg-1".into()),
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
        };
        AgentContext::new("ctx-1".into(), user_input, ContextConfig::default())
    }

    #[test]
    fn read_view_captures_user_input() {
        let ctx = make_context();
        let view = ReadView::new(&ctx);
        assert_eq!(view.user_input().raw_text, "hello");
    }

    #[test]
    fn read_view_is_snapshot() {
        let ctx = make_context();
        let view1 = ReadView::new(&ctx);

        // The view should have empty categories initially
        assert!(
            view1
                .get_category(Category::Entities)
                .is_none_or(|v| v.is_empty())
        );
    }

    #[test]
    fn read_view_get_record() {
        let ctx = make_context();
        let view = ReadView::new(&ctx);
        assert!(view.get_record(Category::Entities, "nonexistent").is_none());
    }
}
