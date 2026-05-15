//! 数据质量服务（Mock 占位）

use anyhow::Result;

pub struct QualityService;

impl QualityService {
    pub fn new() -> Self {
        Self
    }
}

impl Default for QualityService {
    fn default() -> Self {
        Self::new()
    }
}
