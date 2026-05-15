//! Azkaban 服务（Mock 占位）

use anyhow::Result;

pub struct AzkabanService;

impl AzkabanService {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AzkabanService {
    fn default() -> Self {
        Self::new()
    }
}
