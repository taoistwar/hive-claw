//! T109 [US12] Skill prompt contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T109
//! ("在 `crates/hivegui/tests/skill_prompt.rs` 编写显式∪always 去重、
//! 稳定顺序、system prompt 注入和不注册为 Tool 的测试").
//!
//! Public boundary this test exercises:
//!   - `hivegui::agent::agent_content::TurnContent`
//!   - `hivegui::agent::agent_content::SkillDescriptor`
//!   - `hivegui::agent::agent_content::ToolDescriptor`
//!   - `hivegui::agent::agent_content::CapabilityDescriptor`
//!
//! T109 is a Red task. These tests encode the required shape now;
//! T110 and T112/T113 implementation tasks should keep them Green.

use hivegui::agent::agent_content::{
    CapabilityDescriptor, SkillDescriptor, ToolDescriptor, TurnContent,
};

#[test]
fn skill_prompt_union_keeps_stable_insertion_order_and_dedups() {
    let explicit = [
        SkillDescriptor::new("s_alpha", "alpha body"),
        SkillDescriptor::new("s_beta", "beta body"),
    ];
    let always = [
        SkillDescriptor::new("s_beta", "beta body duplicate"),
        SkillDescriptor::new("s_gamma", "gamma body"),
    ];

    let content = TurnContent::new().with_skills(explicit, always);

    assert_eq!(content.skills().len(), 3);
    assert_eq!(content.skills()[0].identifier, "s_alpha");
    assert_eq!(content.skills()[1].identifier, "s_beta");
    assert_eq!(content.skills()[2].identifier, "s_gamma");
    assert_eq!(content.skills()[0].content, "alpha body");
    assert_eq!(content.skills()[1].content, "beta body");
    assert_eq!(content.skills()[2].content, "gamma body");
}

#[test]
fn system_prompt_is_injected_unchanged() {
    let prompt = "System prompt: prefer JSON-only answers.";
    let content = TurnContent::new().with_system_prompt(prompt);
    assert_eq!(content.system_prompt(), prompt);
}

#[test]
fn skills_and_tools_use_disjoint_prompt_channels() {
    let content = TurnContent::new()
        .with_skill(SkillDescriptor::new("s_alpha", "alpha"))
        .with_tool(ToolDescriptor::new("tool_beta", "HTTP tool", Vec::new()))
        .with_capability(CapabilityDescriptor::new("canary", false));

    assert_eq!(content.skills().len(), 1);
    assert_eq!(content.skills()[0].identifier, "s_alpha");
    assert_eq!(content.tools().len(), 1);
    assert_eq!(content.tools()[0].identifier, "tool_beta");
    assert_eq!(content.capabilities().len(), 1);
    assert_eq!(content.capabilities()[0].name, "canary");
}
