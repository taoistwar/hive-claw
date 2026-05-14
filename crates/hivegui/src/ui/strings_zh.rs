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
