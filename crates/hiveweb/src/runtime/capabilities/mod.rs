//! Capability handlers — 每个 capability 一个 handler 文件 (T095..T101 / US4)。
//!
//! 入口在 `runtime::capability::dispatch`，由 Plugin 侧 `host_call(envelope)` 触发。
//! Handler 不直接做权限检查；调用前 dispatcher 已通过 Agent.permissions 过滤。

pub mod db;
pub mod fs;
pub mod llm;
pub mod network_http;
pub mod rate_limit;
pub mod s3;
pub mod secret;
pub mod utility;

/// Internal, typed classification for capability handler failures.
///
/// Private handler details are discarded at this boundary because they may
/// contain provider responses, URLs, paths, SQL or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityFailureKind {
    InvalidArguments,
    Timeout,
    ModelPresetUnknown,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityFailure {
    kind: CapabilityFailureKind,
}

impl CapabilityFailure {
    pub fn failed(_private_detail: impl Into<String>) -> Self {
        Self {
            kind: CapabilityFailureKind::Failed,
        }
    }

    pub fn invalid_arguments() -> Self {
        Self {
            kind: CapabilityFailureKind::InvalidArguments,
        }
    }

    pub fn timeout() -> Self {
        Self {
            kind: CapabilityFailureKind::Timeout,
        }
    }

    pub fn model_preset_unknown() -> Self {
        Self {
            kind: CapabilityFailureKind::ModelPresetUnknown,
        }
    }

    pub fn kind(self) -> CapabilityFailureKind {
        self.kind
    }

    pub fn audit_kind(self) -> Option<&'static str> {
        match self.kind {
            CapabilityFailureKind::InvalidArguments => Some("invalid_capability_args"),
            CapabilityFailureKind::Timeout => Some("capability_timeout"),
            CapabilityFailureKind::ModelPresetUnknown => Some("model_preset_unknown"),
            CapabilityFailureKind::Failed => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CapabilityFailure, CapabilityFailureKind};

    #[test]
    fn model_preset_unknown_has_a_dedicated_failure_kind() {
        let failure = CapabilityFailure::model_preset_unknown();

        assert_eq!(failure.kind(), CapabilityFailureKind::ModelPresetUnknown);
        assert_eq!(failure.audit_kind(), Some("model_preset_unknown"));
    }
}
