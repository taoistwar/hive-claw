//! Response types for AgentContext.
//!
//! Defines the structured response payload that an agent produces at the
//! end of execution, including text, extension content (cards, images, etc.),
//! object references, and suggestions.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Types of extension content that can be included in a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExtensionType {
    /// Card content (e.g., game card, info card)
    Card,
    /// Image content
    Image,
    /// Suggested follow-up questions
    Suggestion,
    /// External link
    Link,
    /// Action button
    Button,
    /// Tabular data
    Table,
    /// Chart/graph visualization
    Chart,
    /// Reference to a business object
    ObjectRef,
}

impl std::fmt::Display for ExtensionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtensionType::Card => write!(f, "card"),
            ExtensionType::Image => write!(f, "image"),
            ExtensionType::Suggestion => write!(f, "suggestion"),
            ExtensionType::Link => write!(f, "link"),
            ExtensionType::Button => write!(f, "button"),
            ExtensionType::Table => write!(f, "table"),
            ExtensionType::Chart => write!(f, "chart"),
            ExtensionType::ObjectRef => write!(f, "object_ref"),
        }
    }
}

/// A single piece of extension content.
///
/// `content_type` is validated via the enum; all other extension fields
/// (id, data, reply, render_hints, etc.) are transparently stored in `raw`
/// and forwarded to the API without modification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionContent {
    /// The type of this extension content (validated)
    pub content_type: ExtensionType,
    /// Complete extension payload — all fields except content_type.
    /// Transparent pass-through to API consumers.
    pub raw: serde_json::Value,
}

impl ExtensionContent {
    /// Create a new `ExtensionContent`.
    /// `raw` should contain all extension fields (id, data, reply, etc.);
    /// any `content_type` inside `raw` is ignored in favour of the validated param.
    pub fn new(content_type: ExtensionType, raw: serde_json::Value) -> Self {
        Self { content_type, raw }
    }

    /// Produce the merged API representation: content_type string + all raw fields.
    pub fn to_api_value(&self) -> serde_json::Value {
        let mut obj = match &self.raw {
            serde_json::Value::Object(m) => m.clone(),
            _ => serde_json::Map::new(),
        };
        obj.insert(
            "content_type".into(),
            serde_json::Value::String(self.content_type.to_string()),
        );
        serde_json::Value::Object(obj)
    }
}

/// A reference to a business object produced or referenced during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectRef {
    /// Type of the referenced object (e.g., "ticket", "order", "document")
    pub object_type: String,
    /// Unique identifier of the object
    pub object_id: String,
    /// Human-readable display name
    pub display_name: String,
    /// Additional metadata (URL, status, etc.)
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// The final response payload produced by an agent execution.
///
/// Contains the text response plus any structured extensions, object
/// references, and suggested follow-up actions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponsePayload {
    /// Primary text response
    pub text: String,
    /// Extension content (cards, images, etc.)
    #[serde(default)]
    pub extensions: Vec<ExtensionContent>,
    /// Referenced business objects
    #[serde(default)]
    pub object_refs: Vec<ObjectRef>,
    /// Suggested follow-up questions/actions
    #[serde(default)]
    pub suggestions: Vec<String>,
}

impl ResponsePayload {
    /// Create a new `ResponsePayload` with the given text.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            extensions: Vec::new(),
            object_refs: Vec::new(),
            suggestions: Vec::new(),
        }
    }

    /// Add extension content.
    pub fn with_extension(mut self, content: ExtensionContent) -> Self {
        self.extensions.push(content);
        self
    }

    /// Add an object reference.
    pub fn with_object_ref(mut self, obj: ObjectRef) -> Self {
        self.object_refs.push(obj);
        self
    }

    /// Add a suggestion.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestions.push(suggestion.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_payload_builder() {
        let payload = ResponsePayload::new("Hello!")
            .with_extension(ExtensionContent::new(
                ExtensionType::Card,
                serde_json::json!({"id": "test_card", "data": {"title": "Game Card"}}),
            ))
            .with_suggestion("Tell me more");

        assert_eq!(payload.text, "Hello!");
        assert_eq!(payload.extensions.len(), 1);
        assert_eq!(payload.extensions[0].content_type, ExtensionType::Card);
        assert_eq!(payload.suggestions.len(), 1);
    }

    #[test]
    fn extension_content_to_api_value() {
        let ext = ExtensionContent::new(
            ExtensionType::Image,
            serde_json::json!({"id": "test", "data": {"url": "http://example.com/img.png"}, "render_hints": {"width": "800", "height": "600"}}),
        );
        let api = ext.to_api_value();
        assert_eq!(api["content_type"], "image");
        assert_eq!(api["id"], "test");
        assert_eq!(api["data"]["url"], "http://example.com/img.png");
        assert_eq!(api["render_hints"]["width"], "800");
    }

    #[test]
    fn object_ref_serialization() {
        let obj = ObjectRef {
            object_type: "ticket".into(),
            object_id: "TKT-123".into(),
            display_name: "Bug Report #123".into(),
            metadata: [("status".into(), "open".into())].into_iter().collect(),
        };

        let json = serde_json::to_string(&obj).unwrap();
        let parsed: ObjectRef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.object_id, "TKT-123");
    }

    #[test]
    fn extension_type_display() {
        assert_eq!(ExtensionType::Card.to_string(), "card");
        assert_eq!(ExtensionType::ObjectRef.to_string(), "object_ref");
    }
}
