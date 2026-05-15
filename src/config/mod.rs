//! 配置模块

use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

/// 应用配置
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AppConfig {
    pub azkaban: AzkabanConfig,
    pub hive_metastore: HiveMetastoreConfig,
    pub ldap: LdapConfig,
    pub git: GitConfig,
    pub ai: AiConfig,
    pub cache: CacheConfig,
    pub quality: Option<QualityConfig>,
}

/// Azkaban 配置
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AzkabanConfig {
    pub host: String,
    pub username: String,
    pub password: String,
}

/// Hive Metastore 配置
#[derive(Debug, Deserialize, Clone, Default)]
pub struct HiveMetastoreConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
}

/// LDAP 配置
#[derive(Debug, Deserialize, Clone, Default)]
pub struct LdapConfig {
    pub host: String,
    pub base_dn: String,
}

/// Git 配置
#[derive(Debug, Deserialize, Clone, Default)]
pub struct GitConfig {
    pub remote: String,
    pub branch_prefix: String,
    pub username: String,
}

/// AI 配置
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AiConfig {
    pub provider: String,
    pub model: String,
    pub api_key: String,
}

/// 缓存配置
#[derive(Debug, Deserialize, Clone, Default)]
pub struct CacheConfig {
    pub metastore_ttl_seconds: u64,
}

/// 质量配置（可选）
#[derive(Debug, Deserialize, Clone, Default)]
pub struct QualityConfig {
    pub alert_email: Option<String>,
    pub default_threshold: Option<f32>,
}

/// 加载配置
pub fn load_config() -> Result<AppConfig> {
    let config_path = Path::new("config.toml");
    
    if !config_path.exists() {
        // 如果配置文件不存在，返回默认配置
        return Ok(get_default_config());
    }
    
    let config_str = std::fs::read_to_string(config_path)?;
    let config: AppConfig = toml::from_str(&config_str)?;
    
    Ok(config)
}

/// 获取默认配置
fn get_default_config() -> AppConfig {
    AppConfig {
        azkaban: AzkabanConfig {
            host: String::new(),
            username: String::new(),
            password: String::new(),
        },
        hive_metastore: HiveMetastoreConfig {
            host: String::new(),
            port: 3306,
            database: String::from("hive"),
            username: String::new(),
            password: String::new(),
        },
        ldap: LdapConfig {
            host: String::new(),
            base_dn: String::new(),
        },
        git: GitConfig {
            remote: String::new(),
            branch_prefix: String::from("task/"),
            username: String::from("developer"),
        },
        ai: AiConfig {
            provider: String::from("openai"),
            model: String::from("gpt-4o"),
            api_key: String::new(),
        },
        cache: CacheConfig {
            metastore_ttl_seconds: 3600,
        },
        quality: None,
    }
}
