//! T125 agent content helpers. The runtime uses these to build
//! immutable content snapshots per turn (e.g. the resolved tool
//! list, the resolved capability set, and the per-turn system
//! prompt). The T125 contract says content must be immutable for
//! the duration of a turn; the helpers here enforce that by
//! returning owned values that cannot be borrowed back to the
//! store.

#![warn(missing_docs)]

use std::collections::BTreeSet;

/// Resolved tool description for one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    /// Stable tool identifier.
    pub identifier: String,
    /// Optional display name.
    pub name: String,
    /// Capability names the tool requires.
    pub requires: Vec<String>,
}

impl ToolDescriptor {
    /// Construct a new tool descriptor.
    pub fn new(
        identifier: impl Into<String>,
        name: impl Into<String>,
        requires: Vec<String>,
    ) -> Self {
        Self {
            identifier: identifier.into(),
            name: name.into(),
            requires,
        }
    }
}

/// Resolved skill content for one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDescriptor {
    /// Stable skill identifier.
    pub identifier: String,
    /// Skill body.
    pub content: String,
}

impl SkillDescriptor {
    /// Construct a new skill descriptor.
    pub fn new(identifier: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            identifier: identifier.into(),
            content: content.into(),
        }
    }
}

/// Resolved capability descriptor for one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDescriptor {
    /// Stable capability name.
    pub name: String,
    /// Whether the capability is dangerous.
    pub is_dangerous: bool,
}

impl CapabilityDescriptor {
    /// Construct a new capability descriptor.
    pub fn new(name: impl Into<String>, is_dangerous: bool) -> Self {
        Self {
            name: name.into(),
            is_dangerous,
        }
    }
}

/// Immutable content bundle for one turn. The T125 runtime hands
/// this to the LLM/UI layer; the caller cannot mutate it.
#[derive(Debug, Clone, Default)]
pub struct TurnContent {
    tools: Vec<ToolDescriptor>,
    skills: Vec<SkillDescriptor>,
    capabilities: Vec<CapabilityDescriptor>,
    system_prompt: String,
}

impl TurnContent {
    /// Build a new empty turn content.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Add a tool descriptor.
    pub fn with_tool(mut self, tool: ToolDescriptor) -> Self {
        if !self.tools.iter().any(|t| t.identifier == tool.identifier) {
            self.tools.push(tool);
        }
        self
    }

    /// Add a skill descriptor.
    pub fn with_skill(mut self, skill: SkillDescriptor) -> Self {
        if !self.skills.iter().any(|s| s.identifier == skill.identifier) {
            self.skills.push(skill);
        }
        self
    }

    /// Add a capability descriptor.
    pub fn with_capability(mut self, capability: CapabilityDescriptor) -> Self {
        if !self.capabilities.iter().any(|c| c.name == capability.name) {
            self.capabilities.push(capability);
        }
        self
    }

    /// Borrow the resolved tools.
    pub fn tools(&self) -> &[ToolDescriptor] {
        &self.tools
    }

    /// Borrow the resolved skills.
    pub fn skills(&self) -> &[SkillDescriptor] {
        &self.skills
    }

    /// Borrow the resolved capabilities.
    pub fn capabilities(&self) -> &[CapabilityDescriptor] {
        &self.capabilities
    }

    /// Borrow the system prompt.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Set of capability names required by the resolved tools.
    pub fn required_capabilities(&self) -> BTreeSet<String> {
        self.tools
            .iter()
            .flat_map(|t| t.requires.iter().cloned())
            .collect()
    }
}
