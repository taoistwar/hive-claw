//! `nanobot onboard` — initialise config + workspace.
//!
//! Mirrors `nanobot/cli/commands.py::onboard`. The interactive wizard is
//! deferred (the Python `nanobot.cli.onboard` is 1100+ lines of
//! questionary-based UX); this command sticks to the non-wizard flow and
//! prints the same "Next steps" hints.

use std::path::{Path, PathBuf};

use config::schema::Config;
use config::{get_config_path, paths::get_workspace_path, set_config_path};
use serde_json::Value;

use crate::runtime::expand_tilde;

/// Args for the onboard command.
pub struct OnboardArgs {
    pub workspace: Option<PathBuf>,
    pub config: Option<PathBuf>,
    /// Overwrite an existing config with defaults instead of refreshing it.
    pub overwrite: bool,
}

pub async fn run(args: OnboardArgs) -> Result<(), String> {
    let config_path = match args.config.as_deref() {
        Some(p) => {
            let resolved = expand_tilde(p);
            set_config_path(&resolved);
            println!("Using config: {}", resolved.display());
            resolved
        }
        None => get_config_path(),
    };

    let mut cfg = if config_path.exists() {
        if args.overwrite {
            let cfg = apply_workspace_override(Config::default(), args.workspace.as_deref());
            cfg.save_config(Some(&config_path)).map_err(|e| e.to_string())?;
            println!("✓ Config reset to defaults at {}", config_path.display());
            cfg
        } else {
            let loaded = Config::from_config(Some(&config_path));
            let cfg = apply_workspace_override(loaded, args.workspace.as_deref());
            cfg.save_config(Some(&config_path)).map_err(|e| e.to_string())?;
            println!(
                "✓ Config refreshed at {} (existing values preserved)",
                config_path.display()
            );
            cfg
        }
    } else {
        let cfg = apply_workspace_override(Config::default(), args.workspace.as_deref());
        cfg.save_config(Some(&config_path)).map_err(|e| e.to_string())?;
        println!("✓ Created config at {}", config_path.display());
        cfg
    };

    onboard_plugins(&config_path);

    let workspace_path = get_workspace_path(Some(&cfg.agents.defaults.workspace));
    if !workspace_path.exists() {
        std::fs::create_dir_all(&workspace_path).map_err(|e| e.to_string())?;
        println!("✓ Created workspace at {}", workspace_path.display());
    }
    // Reload to surface any extras merged by `onboard_plugins`.
    cfg = Config::from_config(Some(&config_path));

    let mut agent_cmd = "nanobot agent -m \"Hello!\"".to_string();
    let mut gateway_cmd = "nanobot gateway".to_string();
    if config_path != get_config_path() {
        agent_cmd.push_str(&format!(" --config {}", config_path.display()));
        gateway_cmd.push_str(&format!(" --config {}", config_path.display()));
    }

    println!();
    println!("nanobot is ready!");
    println!();
    println!("Next steps:");
    println!("  1. Add your API key to {}", config_path.display());
    println!("     Get one at: https://openrouter.ai/keys");
    println!("  2. Chat: {agent_cmd}");
    println!("  3. Start gateway: {gateway_cmd}");
    println!();
    println!("Want Telegram/WhatsApp? See: https://github.com/HKUDS/nanobot#-chat-apps");

    let _ = cfg; // suppress unused
    Ok(())
}

fn apply_workspace_override(mut cfg: Config, workspace: Option<&Path>) -> Config {
    if let Some(ws) = workspace {
        cfg.agents.defaults.workspace = expand_tilde(ws).to_string_lossy().into_owned();
    }
    cfg
}

/// Inject default sub-config blocks for any "discovered" channels.
///
/// The Python original walks an entry-point registry. The Rust port has
/// no channel implementations yet, so this routine is currently a no-op
/// hook that simply ensures the `channels` object exists in JSON form so
/// users can start filling in channel sections by hand.
fn onboard_plugins(config_path: &Path) {
    let Ok(text) = std::fs::read_to_string(config_path) else {
        return;
    };
    let Ok(mut data) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let root = match data.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    root.entry("channels")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Ok(out) = serde_json::to_string_pretty(&data) {
        let _ = std::fs::write(config_path, out);
    }
}
