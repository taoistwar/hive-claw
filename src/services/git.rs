//! Git 服务（Mock 占位）

use anyhow::Result;

pub struct GitService;

impl GitService {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GitService {
    fn default() -> Self {
        Self::new()
    }
}
