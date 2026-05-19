//! 大数据离线分析 AI Agent
//! 
//! 一个基于 Rust 的桌面应用，用于管理 Azkaban 离线分析任务。

mod gui;
mod services;
mod models;
mod config;
mod cli;

use anyhow::Result;
use tracing_subscriber;
use std::env;

fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting Offline Analysis Agent...");

    // 处理命令行参数
    let args: Vec<String> = env::args().collect();
    
    if args.len() > 1 {
        match args[1].as_str() {
            "--test-connection" | "-t" => {
                // 测试数据库和 API 连接
                return tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(cli::run_all_tests());
            }
            "--cli" => {
                // CLI 模式（预留）
                println!("CLI mode - coming soon");
                return Ok(());
            }
            "--gui" => {
                // GUI 模式（默认）
                let gui_type = args.get(2).map(|s| s.as_str()).unwrap_or("tui");
                return run_gui(gui_type);
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            _ => {
                // 未知参数，运行 GUI
                return run_gui("tui");
            }
        }
    }
    
    // 默认运行 TUI
    run_gui("tui")
}

fn run_gui(gui_type: &str) -> Result<()> {
    // 加载配置
    let config = config::load_config()?;
    tracing::info!("Configuration loaded from config.toml");

    match gui_type {
        "tui" => {
            tracing::info!("Starting TUI GUI...");
            match gui::run_app(config) {
                Ok(_) => {
                    tracing::info!("Application exited normally");
                }
                Err(e) => {
                    tracing::error!("Application error: {}", e);
                    return Err(e);
                }
            }
        }
        "gpui" => {
            println!("GPUI GUI is experimental and not available in this build.");
            println!("To enable GPUI:");
            println!("  1. Edit Cargo.toml and uncomment gpui dependency");
            println!("  2. Install system dependencies:");
            println!("     apt-get install libwayland-dev libxkbcommon-dev libgtk-3-dev libclang-dev");
            println!("  3. Rebuild: cargo build --release");
        }
        _ => {
            println!("Unknown GUI type: {}", gui_type);
            println!("Available: tui, gpui");
        }
    }

    Ok(())
}

fn print_help() {
    println!("离线分析 AI Agent - Azkaban 任务管理工具");
    println!();
    println!("用法:");
    println!("  offline-analysis-agent [选项]");
    println!();
    println!("选项:");
    println!("  --gui <type>, -g     运行 GUI (默认：tui)");
    println!("                       可选：tui, gpui");
    println!("  --test-connection, -t  测试数据库和 API 连接");
    println!("  --cli                CLI 模式（开发中）");
    println!("  --help, -h           显示帮助信息");
    println!();
    println!("示例:");
    println!("  # 运行 TUI 界面");
    println!("  ./offline-analysis-agent");
    println!();
    println!("  # 测试数据库连接");
    println!("  ./offline-analysis-agent --test-connection");
    println!();
    println!("  # 运行 GPUI（如果有图形环境）");
    println!("  ./offline-analysis-agent --gui gpui");
}
