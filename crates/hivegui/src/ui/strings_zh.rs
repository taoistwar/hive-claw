//! Canonical Simplified-Chinese (zh-CN) copy for every user-facing string
//! in HiveGUI v1. Concentrated in a single module so review can verify
//! FR-012a in one place. Developer-facing strings (log lines, exception
//! messages) MAY remain in English and live elsewhere.

pub const APP_TITLE: &str = "HiveGUI";
pub const HOME_TITLE: &str = "首页";

pub const CONVERSATION_ENTRY: &str = "与 HiveClaw 对话";
pub const CONVERSATION_TITLE: &str = "会话";
pub const SEND_PLACEHOLDER: &str = "输入你的问题…";
pub const SEND_BUTTON: &str = "发送";
pub const RETRY_BUTTON: &str = "重试";
pub const DISMISS_BUTTON: &str = "关闭";

pub const IN_PROGRESS: &str = "等待 HiveClaw 回复…";

pub const SPEAKER_USER: &str = "你";
pub const SPEAKER_ASSISTANT: &str = "HiveClaw";

pub const DAY_PLUS_ONE_LABEL: &str = "Day+1 工具";
pub const HOUR_PLUS_ONE_LABEL: &str = "Hour+1 工具";

pub const EMPTY_TOOLS: &str = "暂无工具";
pub const EMPTY_TOOLS_LOAD_FAILED: &str = "工具加载失败，请稍后重试";

pub const ERR_HIVECLAW_UNREACHABLE: &str = "HiveClaw 不可达，请检查服务是否运行";
pub const ERR_REPLY_FAILED: &str = "回复失败，请点击重试";

// Phase 8 / User Story 5 — conversation input + file attachments.

/// Label of the file-attachment affordance next to the send button (FR-007b).
pub const ATTACH_BUTTON: &str = "添加文件";

/// Per-chip "remove this attachment" affordance.
pub const ATTACHMENT_REMOVE: &str = "移除";

/// Error rendered next to the input surface when a single picked file
/// exceeds the per-file budget (data-model invariant A1). Format with the
/// filename, e.g. via `format!("{}: {}", ERR_FILE_TOO_LARGE, name)`.
pub const ERR_FILE_TOO_LARGE: &str = "文件超过 1 MiB 限制";

/// Error when the running sum of pending attachments would exceed the
/// per-turn total budget (data-model invariant A1).
pub const ERR_TOTAL_TOO_LARGE: &str = "附件总大小超过 4 MiB 限制";

/// Error when the engineer would attach a 9th file on a single pending
/// turn (data-model invariant A1: count ≤ 8).
pub const ERR_TOO_MANY_ATTACHMENTS: &str = "单次会话最多附加 8 个文件";

/// Error when `mime_guess` cannot resolve a media type for the picked file
/// (data-model invariant A2).
pub const ERR_UNSUPPORTED_MIME: &str = "无法识别的文件类型";

/// Error when the picked file cannot be read from disk (permission /
/// missing / I/O failure).
pub const ERR_FILE_READ_FAILED: &str = "无法读取文件";

/// Error when the picked file is already attached to the current turn.
pub const ERR_FILE_ALREADY_ATTACHED: &str = "该文件已添加";
