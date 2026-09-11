//! Ngy Prompt Studio 应用入口（提示词工程）。
//!
//! 复用 `hivegui::startup::launch` 完成配置解析、日志与 Tokio 运行时初始化，
//! 然后启动聚焦的提示词工程界面（[`hivegui::ui::app::run_prompt_studio`）：
//! LLM 提示词调试、LLM 配置（Model / Preset / Provider）、全局配置、分类管理。
//!
//! 传入 [`AppIdentity::NGY_PROMPT_STUDIO`]，使本应用使用独立的数据目录
//! （`{data_local_dir}/ngy_prompt_studio`）与独立的日志文件
//! （`ngy_prompt_studio.log`），不与其他 HiveGUI 入口（`hivegui`）共享。

use std::process::ExitCode;

use hivegui::config::AppIdentity;
use hivegui::startup;
use hivegui::ui::app;

fn main() -> ExitCode {
    startup::launch(AppIdentity::NGY_PROMPT_STUDIO, |cfg| {
        app::run_prompt_studio(cfg)
    })
}
