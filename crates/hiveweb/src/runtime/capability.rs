//! Capability 静态注册表（FR-001 / FR-003 / data-model §6）
//!
//! Capability 是宿主侧零信任默认 deny 的能力点；Agent 的 permissions 必须显式
//! 列出某 capability 才允许 Plugin 在调用时使用它。注册表 = 代码侧真值源；DB
//! `capabilities` 表 = 启动期 upsert 的镜像（V008 + V018 seed）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 11 个 capability 名常量（与 data-model §V018 / §6 完全对齐）
pub const NETWORK_HTTP: &str = "network.http";
pub const FS_READ: &str = "fs.read";
pub const FS_WRITE: &str = "fs.write";
pub const S3_READ: &str = "s3.read";
pub const S3_WRITE: &str = "s3.write";
pub const DB_QUERY: &str = "db.query";
pub const DB_EXECUTE: &str = "db.execute";
pub const LLM_INVOKE: &str = "llm.invoke";
pub const SECRET_GET: &str = "secret.get";
pub const TIME_NOW: &str = "time.now";
pub const LOG_EMIT: &str = "log.emit";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub name: &'static str,
    pub description: &'static str,
    pub is_dangerous: bool,
}

pub const CAPABILITIES: &[Capability] = &[
    Capability { name: NETWORK_HTTP, description: "HTTP/HTTPS access (allowlisted hosts; SSRF-blocked)", is_dangerous: true },
    Capability { name: FS_READ,      description: "/tmp/plugin/ 内文件读", is_dangerous: false },
    Capability { name: FS_WRITE,     description: "/tmp/plugin/ 内文件写", is_dangerous: false },
    Capability { name: S3_READ,      description: "Rustfs 桶 GET", is_dangerous: false },
    Capability { name: S3_WRITE,     description: "Rustfs 桶 PUT / DELETE", is_dangerous: false },
    Capability { name: DB_QUERY,     description: "宿主预注册命名 SELECT 查询", is_dangerous: false },
    Capability { name: DB_EXECUTE,   description: "宿主预注册命名 DML（永不自由 SQL）", is_dangerous: true },
    Capability { name: LLM_INVOKE,   description: "LLM 调用（走 Agent.model_preset 解析）", is_dangerous: false },
    Capability { name: SECRET_GET,   description: "allowlist 内的密钥读取", is_dangerous: true },
    Capability { name: TIME_NOW,     description: "服务器当前时间", is_dangerous: false },
    Capability { name: LOG_EMIT,     description: "结构化日志写入（rate-limited）", is_dangerous: false },
];

#[derive(Debug, Clone)]
pub struct CapabilityRegistry {
    index: HashMap<&'static str, &'static Capability>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        let mut index = HashMap::with_capacity(CAPABILITIES.len());
        for cap in CAPABILITIES {
            index.insert(cap.name, cap);
        }
        Self { index }
    }

    pub fn lookup(&self, name: &str) -> Option<&'static Capability> {
        self.index.get(name).copied()
    }

    pub fn is_dangerous(&self, name: &str) -> bool {
        self.lookup(name).map(|c| c.is_dangerous).unwrap_or(false)
    }

    pub fn all(&self) -> &'static [Capability] {
        CAPABILITIES
    }
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}
