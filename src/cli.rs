//! 配置验证和数据库连接测试工具

use crate::config::{load_config, AppConfig};
use crate::services::metadata::MetadataService;
use crate::services::azkaban::AzkabanService;
use anyhow::Result;
use colored::*;

/// 测试数据库连接
pub async fn test_database_connection(config: &AppConfig) -> Result<()> {
    println!("\n{}", "测试 Hive Metastore 数据库连接 ...".cyan());
    
    let mut metadata_service = MetadataService::new(config.clone());
    
    match metadata_service.initialize().await {
        Ok(()) => {
            println!("{} 数据库连接成功", "✓".green());
            
            // 尝试获取数据库列表
            match metadata_service.list_databases().await {
                Ok(databases) => {
                    println!("{} 获取到 {} 个数据库:", "✓".green(), databases.len());
                    for db in databases.iter().take(10) {
                        println!("  - {}", db.name.bold());
                    }
                    if databases.len() > 10 {
                        println!("  ... 还有 {} 个", databases.len() - 10);
                    }
                }
                Err(e) => {
                    println!("{} 获取数据库列表失败：{}", "✗".red(), e);
                }
            }
            
            Ok(())
        }
        Err(e) => {
            println!("{} 数据库连接失败：{}", "✗".red(), e);
            println!("\n{}", "请检查:".yellow());
            println!("  1. Hive Metastore MySQL 服务是否运行");
            println!("  2. config.toml 中的连接配置是否正确");
            println!("  3. 网络连接和防火墙设置");
            Err(e)
        }
    }
}

/// 测试 Azkaban 连接
pub async fn test_azkaban_connection(config: &AppConfig) -> Result<()> {
    println!("\n{}", "测试 Azkaban API 连接 ...".cyan());
    
    let azkaban_service = AzkabanService::new(config.clone());
    
    // 初始化客户端
    if let Err(e) = azkaban_service.initialize().await {
        println!("{} Azkaban 初始化失败：{}", "✗".red(), e);
        return Err(e);
    }
    
    match azkaban_service.login().await {
        Ok(()) => {
            println!("{} Azkaban 登录成功", "✓".green());
            
            // 尝试获取项目列表
            match azkaban_service.list_projects().await {
                Ok(projects) => {
                    println!("{} 获取到 {} 个项目:", "✓".green(), projects.len());
                    for project in projects.iter().take(10) {
                        println!("  - {}", project.name.bold());
                    }
                    if projects.len() > 10 {
                        println!("  ... 还有 {} 个", projects.len() - 10);
                    }
                }
                Err(e) => {
                    println!("{} 获取项目列表失败：{}", "✗".red(), e);
                }
            }
            
            Ok(())
        }
        Err(e) => {
            println!("{} Azkaban 登录失败：{}", "✗".red(), e);
            println!("\n{}", "请检查:".yellow());
            println!("  1. Azkaban Web 服务是否运行");
            println!("  2. config.toml 中的用户名密码是否正确");
            println!("  3. 网络连接和防火墙设置");
            Err(e)
        }
    }
}

/// 验证配置文件
pub fn validate_config(config: &AppConfig) -> Result<()> {
    println!("\n{}", "验证配置文件 ...".cyan());
    
    let mut valid = true;
    
    // 检查 Hive Metastore 配置
    if config.hive_metastore.host.is_empty() {
        println!("{} Hive Metastore host 未配置", "✗".red());
        valid = false;
    } else {
        println!("{} Hive Metastore: {}:{}", "✓".green(), 
                 config.hive_metastore.host, config.hive_metastore.port);
    }
    
    // 检查 Azkaban 配置
    if config.azkaban.host.is_empty() {
        println!("{} Azkaban host 未配置", "✗".red());
        valid = false;
    } else {
        println!("{} Azkaban: {}", "✓".green(), config.azkaban.host);
    }
    
    // 检查 AI 配置
    if config.ai.api_key.is_empty() {
        println!("{} AI API Key 未配置", "✗".red());
        valid = false;
    } else {
        let key_preview = format!("{}...", &config.ai.api_key[..std::cmp::min(8, config.ai.api_key.len())]);
        println!("{} AI Provider: {} ({})", "✓".green(), config.ai.provider, key_preview);
    }
    
    // 检查 Git 配置
    if config.git.remote.is_empty() {
        println!("{} Git remote 未配置", "✗".red());
        valid = false;
    } else {
        println!("{} Git Remote: {}", "✓".green(), config.git.remote);
    }
    
    if valid {
        println!("\n{}", "配置验证通过".green());
        Ok(())
    } else {
        println!("\n{}", "配置验证失败，请检查 config.toml".red());
        anyhow::bail!("配置验证失败")
    }
}

/// 运行所有测试
pub async fn run_all_tests() -> Result<()> {
    let config = match load_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            println!("{} 加载配置文件失败：{}", "✗".red(), e);
            println!("提示：请复制 config.template.toml 为 config.toml 并修改配置");
            return Err(e);
        }
    };
    
    println!("\n{}", "=".repeat(50));
    println!("{}", "离线分析 AI Agent - 连接测试".bold());
    println!("{}", "=".repeat(50));
    
    // 验证配置
    if let Err(e) = validate_config(&config) {
        return Err(e);
    }
    
    // 测试数据库连接
    let _ = test_database_connection(&config).await;
    
    // 测试 Azkaban 连接
    let _ = test_azkaban_connection(&config).await;
    
    println!("\n{}", "=".repeat(50));
    println!("{}", "测试完成".bold().green());
    println!("{}", "=".repeat(50));
    
    Ok(())
}
