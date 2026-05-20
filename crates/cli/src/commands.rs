//! `clap`-based subcommand dispatcher.
//!
//! The dispatcher itself stays small: each command lives in its own
//! module (`onboard`, `gateway`, `agent_cmd`, `status`) and the heavy
//! lifting (config loading, provider construction, AgentLoop wiring)
//! goes through [`crate::runtime`].

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use api::{ApiAgent, ApiServerConfig};
use clap::{Parser, Subcommand};
use log::info;

use crate::adapter::AgentLoopApi;
use crate::agent_cmd::{self, AgentArgs};
use crate::gateway::{self, GatewayArgs};
use crate::onboard::{self, OnboardArgs};
use crate::runtime::Runtime;
use crate::{LoopBundle, status as status_cmd};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Top-level `nanobot` CLI.
#[derive(Parser, Debug)]
#[command(
    name = "nanobot",
    version = VERSION,
    about = "Personal AI assistant (Rust port)",
    propagate_version = true,
)]
pub struct Cli {
    /// Workspace directory. Defaults to the value in the loaded config.
    #[arg(long, global = true)]
    pub workspace: Option<PathBuf>,

    /// Path to the config file (defaults to `~/.nanobot/config.json`).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print the CLI version and exit.
    Version,

    /// First-time setup: create config + workspace and print next steps.
    Onboard {
        /// Overwrite an existing config with defaults.
        #[arg(long)]
        overwrite: bool,
    },

    /// Show config / workspace / model / provider credential status.
    Status,

    /// Send one prompt or open an interactive REPL.
    Agent {
        /// Single-shot message. Use `-` to read from stdin.
        #[arg(short = 'm', long)]
        message: Option<String>,

        /// Override the session id (default `cli:default`).
        #[arg(short = 's', long)]
        session: Option<String>,

        /// (reserved) render replies as Markdown — falls back to plain text.
        #[arg(long, default_value_t = false)]
        markdown: bool,

        /// Forward agent INFO logs to stderr.
        #[arg(long, default_value_t = false)]
        logs: bool,
    },

    /// Backwards-compat alias for `agent --message …`.
    Run {
        #[arg(short, long)]
        message: Option<String>,
    },

    /// Backwards-compat alias for `agent` (REPL).
    Chat,

    /// Start the OpenAI-compatible HTTP API server.
    Serve {
        /// Bind socket address. Overrides config when set.
        #[arg(long)]
        bind: Option<SocketAddr>,

        /// Bind host. Overrides config when set.
        #[arg(long)]
        host: Option<String>,

        /// Bind port. Overrides config when set.
        #[arg(long)]
        port: Option<u16>,

        /// Per-request timeout (seconds). Overrides config when set.
        #[arg(long)]
        timeout: Option<f32>,

        /// Model name advertised by `/v1/models`.
        #[arg(long)]
        model_name: Option<String>,

        /// Enable INFO-level log forwarding.
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },

    /// Long-running gateway: cron + heartbeat + bus loop + /health.
    Gateway {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        /// Enable DEBUG-level log forwarding.
        #[arg(long, default_value_t = false)]
        verbose: bool,
    },

    /// Channel adapter management (status / login).
    Channels {
        #[command(subcommand)]
        sub: ChannelsCommand,
    },

    /// Channel plugin discovery (built-in + configured).
    Plugins {
        #[command(subcommand)]
        sub: PluginsCommand,
    },

    /// Interactive provider login flows.
    Provider {
        #[command(subcommand)]
        sub: ProviderCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProviderCommand {
    /// Trigger an interactive login.
    Login {
        /// `github-copilot` or `openai-codex`.
        name: String,
    },
    /// Print login status for the given provider.
    Status { name: String },
}

#[derive(Subcommand, Debug)]
pub enum ChannelsCommand {
    /// List configured channels and whether they're enabled.
    Status,
    /// Trigger an interactive channel login (e.g. WeChat QR scan).
    Login {
        /// Channel name (e.g. `weixin`, `qq`).
        name: String,
        /// Force re-login even if a saved session exists.
        #[arg(short = 'f', long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum PluginsCommand {
    /// List configured channel plugins.
    List,
}

/// Parse argv + dispatch to the matching handler.
pub async fn dispatch() -> Result<(), String> {
    let cli = Cli::parse();
    let cfg_path = cli.config.clone();
    let ws_path = cli.workspace.clone();

    match cli.command {
        Command::Version => {
            println!("nanobot {VERSION}");
            Ok(())
        }
        Command::Onboard { overwrite } => {
            onboard::run(OnboardArgs {
                workspace: ws_path,
                config: cfg_path,
                overwrite,
            })
            .await
        }
        Command::Status => status_cmd::status(ws_path, cfg_path).await,
        Command::Agent {
            message,
            session,
            markdown,
            logs,
        } => {
            agent_cmd::run(AgentArgs {
                message,
                session,
                workspace: ws_path,
                config: cfg_path,
                markdown,
                logs,
            })
            .await
        }
        Command::Run { message } => {
            agent_cmd::run(AgentArgs {
                message: Some(message.unwrap_or_else(|| "-".into())),
                workspace: ws_path,
                config: cfg_path,
                ..Default::default()
            })
            .await
        }
        Command::Chat => {
            agent_cmd::run(AgentArgs {
                workspace: ws_path,
                config: cfg_path,
                ..Default::default()
            })
            .await
        }
        Command::Serve {
            bind,
            host,
            port,
            timeout,
            model_name,
            verbose,
        } => {
            serve(
                ws_path, cfg_path, bind, host, port, timeout, model_name, verbose,
            )
            .await
        }
        Command::Gateway { host, port, verbose } => {
            gateway::run(GatewayArgs {
                workspace: ws_path,
                config: cfg_path,
                host,
                port,
                verbose,
            })
            .await
        }
        Command::Channels { sub } => match sub {
            ChannelsCommand::Status => status_cmd::channels_status(cfg_path).await,
            ChannelsCommand::Login { name, force } => {
                status_cmd::channels_login(name, force, cfg_path).await
            }
        },
        Command::Plugins { sub } => match sub {
            PluginsCommand::List => status_cmd::plugins_list(cfg_path).await,
        },
        Command::Provider { sub } => match sub {
            ProviderCommand::Login { name } => provider_login(&name).await,
            ProviderCommand::Status { name } => provider_status(&name),
        },
    }
}

// ---------------------------------------------------------------------------
// serve
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn serve(
    workspace: Option<PathBuf>,
    config: Option<PathBuf>,
    bind: Option<SocketAddr>,
    host: Option<String>,
    port: Option<u16>,
    timeout: Option<f32>,
    model_name: Option<String>,
    verbose: bool,
) -> Result<(), String> {
    if verbose {
        let _ = env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Info)
            .try_init();
    }

    let Runtime { config: cfg, .. } =
        Runtime::from_config(config.as_deref(), workspace.as_deref())?;

    let bind: SocketAddr = if let Some(b) = bind {
        b
    } else {
        let h = host.unwrap_or_else(|| cfg.api.host.clone());
        let p = port.unwrap_or(cfg.api.port);
        format!("{h}:{p}")
            .parse()
            .map_err(|e: std::net::AddrParseError| format!("invalid bind: {e}"))?
    };
    if bind.ip().to_string() == "0.0.0.0" {
        eprintln!("warning: serving on 0.0.0.0 exposes the API to the local network.");
    }

    let bundle = LoopBundle::build_agent_loop(&cfg, None).await?;
    let adapter: Arc<dyn ApiAgent> = Arc::new(AgentLoopApi::new(Arc::new(bundle.agent)));

    let secs = timeout.unwrap_or(cfg.api.timeout).max(1.0);
    let server_cfg = ApiServerConfig::new(bind, config::get_media_dir(None))
        .with_model_name(model_name.unwrap_or_else(|| cfg.agents.defaults.model.clone()))
        .with_request_timeout(std::time::Duration::from_secs_f32(secs));
    info!("serving OpenAI-compatible API on http://{bind}");

    api::serve(adapter, server_cfg)
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// provider login / status
// ---------------------------------------------------------------------------

async fn provider_login(name: &str) -> Result<(), String> {
    match name.to_ascii_lowercase().as_str() {
        "github-copilot" | "github_copilot" | "copilot" => {
            let token = providers::login_github_copilot(|m| println!("{m}"))
                .await
                .map_err(|e| format!("github copilot login failed: {e}"))?;
            println!(
                "github copilot: login ok (account_id={})",
                token.account_id.as_deref().unwrap_or("?")
            );
            Ok(())
        }
        "openai-codex" | "openai_codex" | "codex" => {
            // The Rust port doesn't yet ship its own device-code flow for
            // Codex. Fall back to the upstream `codex` CLI which writes
            // `~/.codex/auth.json`; the OpenAI Codex provider already
            // imports that file as a fallback (see `oauth.rs`).
            Err(
                "openai-codex login is not yet implemented in the Rust build. \
                 Run `codex login` (Node.js) and the Rust provider will pick \
                 up the credential from `~/.codex/auth.json`."
                    .into(),
            )
        }
        other => Err(format!("unknown provider: {other}")),
    }
}

fn provider_status(name: &str) -> Result<(), String> {
    use providers::FileTokenStorage;
    match name.to_ascii_lowercase().as_str() {
        "github-copilot" | "github_copilot" | "copilot" => {
            match providers::get_github_copilot_login_status() {
                Some(tok) => println!(
                    "github copilot: logged in (account_id={})",
                    tok.account_id.as_deref().unwrap_or("?")
                ),
                None => println!("github copilot: not logged in"),
            }
            Ok(())
        }
        "openai-codex" | "openai_codex" | "codex" => {
            let storage = FileTokenStorage::new(
                providers::openai_codex::TOKEN_FILENAME,
                providers::openai_codex::TOKEN_APP_NAME,
                true,
            );
            match storage.load() {
                Some(tok) => println!(
                    "openai codex: logged in (account_id={})",
                    tok.account_id.as_deref().unwrap_or("?")
                ),
                None => println!("openai codex: not logged in"),
            }
            Ok(())
        }
        other => Err(format!("unknown provider: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_version() {
        let cli = Cli::try_parse_from(["nanobot", "version"]).unwrap();
        assert!(matches!(cli.command, Command::Version));
    }

    #[test]
    fn parse_agent_with_message() {
        let cli = Cli::try_parse_from(["nanobot", "agent", "-m", "hello"]).unwrap();
        match cli.command {
            Command::Agent { message, .. } => assert_eq!(message.as_deref(), Some("hello")),
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn parse_serve_no_bind_uses_config() {
        let cli = Cli::try_parse_from(["nanobot", "serve"]).unwrap();
        match cli.command {
            Command::Serve {
                bind, port, host, ..
            } => {
                assert!(bind.is_none());
                assert!(port.is_none());
                assert!(host.is_none());
            }
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn parse_provider_login_codex() {
        let cli = Cli::try_parse_from(["nanobot", "provider", "login", "openai-codex"]).unwrap();
        match cli.command {
            Command::Provider {
                sub: ProviderCommand::Login { name },
            } => assert_eq!(name, "openai-codex"),
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn parse_gateway_default() {
        let cli = Cli::try_parse_from(["nanobot", "gateway"]).unwrap();
        assert!(matches!(cli.command, Command::Gateway { .. }));
    }

    #[test]
    fn parse_channels_status() {
        let cli = Cli::try_parse_from(["nanobot", "channels", "status"]).unwrap();
        match cli.command {
            Command::Channels {
                sub: ChannelsCommand::Status,
            } => {}
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn parse_plugins_list() {
        let cli = Cli::try_parse_from(["nanobot", "plugins", "list"]).unwrap();
        match cli.command {
            Command::Plugins {
                sub: PluginsCommand::List,
            } => {}
            _ => panic!("wrong subcommand"),
        }
    }

    #[test]
    fn parse_global_config() {
        let cli = Cli::try_parse_from(["nanobot", "--config", "/tmp/cfg.json", "status"]).unwrap();
        assert_eq!(
            cli.config.as_deref(),
            Some(std::path::Path::new("/tmp/cfg.json"))
        );
    }
}
