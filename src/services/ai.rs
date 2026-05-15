//! AI 服务（Mock 占位）

use anyhow::Result;

pub struct AiService;

impl AiService {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AiService {
    fn default() -> Self {
        Self::new()
    }
}
