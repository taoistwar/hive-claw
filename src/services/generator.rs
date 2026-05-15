//! 任务生成器服务（Mock 占位）

use anyhow::Result;

pub struct GeneratorService;

impl GeneratorService {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GeneratorService {
    fn default() -> Self {
        Self::new()
    }
}
