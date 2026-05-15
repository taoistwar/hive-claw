//! 大数据离线分析 AI Agent
//! 
//! 一个基于 Rust 的桌面应用，用于管理 Azkaban 离线分析任务。

mod gui;
mod services;
mod models;
mod config;

use anyhow::Result;
use tracing_subscriber;

fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting Offline Analysis Agent...");

    // 加载配置
    let config = config::load_config()?;
    tracing::info!("Configuration loaded from config.toml");

    // 启动 GUI 应用
    match gui::run_app(config) {
        Ok(_) => {
            tracing::info!("Application exited normally");
        }
        Err(e) => {
            tracing::error!("Application error: {}", e);
            return Err(e);
        }
    }

    Ok(())
}
