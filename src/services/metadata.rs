//! 元数据服务（Mock 占位）

use anyhow::Result;

pub struct MetadataService;

impl MetadataService {
    pub fn new() -> Self {
        Self
    }
    
    pub async fn list_databases(&self) -> Result<Vec<String>> {
        Ok(vec!["default".to_string(), "ods".to_string(), "dwd".to_string(), "dws".to_string()])
    }
    
    pub async fn list_tables(&self, _db: &str) -> Result<Vec<String>> {
        Ok(vec!["users".to_string(), "orders".to_string(), "products".to_string()])
    }
}

impl Default for MetadataService {
    fn default() -> Self {
        Self::new()
    }
}
