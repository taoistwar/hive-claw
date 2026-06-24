//! `nanobot gateway` — long-running service that wires the agent to
//! the cron scheduler, heartbeat task, channel adapters, and a small
//! `/health` HTTP endpoint.
//!
//! Mirrors the Python `gateway` command. Channel adapters that fail to
//! start are logged but don't tear the whole process down.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use agent::{AgentLoop, Dream, DreamConfig, MemoryDream};
use async_trait::async_trait;
use bus::InboundMessage;
use channels::ChannelManager;
use chrono::Local;
use cron::{CronJob, CronPayload, CronSchedule, JobHandler, PayloadKind, ScheduleKind};
use heartbeat::{HeartbeatConfig as HbCfg, HeartbeatDecider, HeartbeatExecutor, HeartbeatService};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::oneshot;

use crate::{
    LoopBundle,
    runtime::{Runtime, migrate_cron_store},
};
use providers::make_provider;

/// Args for `nanobot gateway`.
#[derive(Debug, Default, Clone)]
pub struct GatewayArgs {
    pub workspace: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub verbose: bool,
}

pub async fn run(args: GatewayArgs) -> Result<(), String> {
    let log_level = if args.verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    let _ = env_logger::Builder::from_default_env()
        .filter_level(log_level)
        .try_init();

    let Runtime { config: cfg, .. } =
        Runtime::from_config(args.config.as_deref(), args.workspace.as_deref())?;

    // Preserve existing single-workspace installs, but keep custom workspaces clean.
    if config::paths::is_default_workspace(Some(cfg.workspace_path())) {
        migrate_cron_store(&cfg);
    }

    let host = args.host.unwrap_or_else(|| cfg.gateway.host.clone());
    let port = args.port.unwrap_or(cfg.gateway.port);
    let bind = format!("{host}:{port}");

    if host == "0.0.0.0" {
        eprintln!(
            "warning: binding 0.0.0.0 exposes the gateway to the local network. \
             Prefer 127.0.0.1 unless you know what you're doing."
        );
    }

    // ---- cron service (no handler yet — wired below once the agent loop exists) ----
    let cron_path = cfg.workspace_path().join("cron").join("jobs.json");
    let cron_svc = Arc::new(::cron::CronService::new(cron_path, None));

    // ---- agent loop ----
    let bundle = LoopBundle::build_agent_loop(&cfg, Some(cron_svc.clone())).await?;
    let agent = Arc::new(bundle.agent);
    let bus = bundle.bus.clone();

    // ---- Dream processor (Phase 1 + Phase 2 memory consolidation) ----
    // Re-uses the same provider/model as the agent loop. Built once and
    // shared with the cron handler so the system::dream job triggers it.
    let dream_provider = make_provider(&cfg)?;
    let dream_model = cfg.agents.defaults.model.clone();

    // Apply Dream config overrides from config file
    let dream_cfg = &cfg.agents.defaults.dream;
    let dream_config = DreamConfig {
        max_batch_size: dream_cfg.max_batch_size as usize,
        max_iterations: dream_cfg.max_iterations,
        max_tool_result_chars: cfg.agents.defaults.max_tool_result_chars as usize,
        annotate_line_ages: dream_cfg.annotate_line_ages,
    };

    let dream = Arc::new(MemoryDream::new(
        cfg.workspace_path(),
        dream_provider,
        dream_model,
        dream_config,
    ));
    if let Err(e) = dream.initialize().await {
        eprintln!("warning: dream initialization failed: {e}");
    }

    // Re-construct the cron service with a real handler now that we have
    // both the agent loop and the dream processor.
    let store_path = cfg.workspace_path().join("cron").join("jobs.json");
    let cron_handler: Arc<dyn JobHandler> = Arc::new(CronAgentHandler {
        agent: agent.clone(),
        dream: dream.clone(),
    });
    let cron_svc = Arc::new(::cron::CronService::new(store_path, Some(cron_handler)));
    cron_svc.start().await;

    // ---- Register the recurring "Dream" system job ----
    register_dream_job(&cron_svc, &cfg).await;

    // ---- channel adapters (must be created before heartbeat) ----
    let channel_mgr: Option<Arc<ChannelManager>> =
        match ChannelManager::new(Arc::new(cfg.clone()), (*bus).clone()) {
            Ok(mgr) => {
                mgr.clone().start_all().await;
                let names = mgr.enabled_channels().await;
                if !names.is_empty() {
                    println!("channels enabled: {}", names.join(", "));
                }
                Some(mgr)
            }
            Err(e) => {
                eprintln!("warning: channel manager init failed: {e}");
                None
            }
        };

    // ---- heartbeat ----
    let hb_cfg = HbCfg {
        workspace: cfg.workspace_path(),
        interval_s: cfg.gateway.heartbeat.interval_s as u64,
        enabled: cfg.gateway.heartbeat.enabled,
        timezone: Some(cfg.agents.defaults.timezone.clone()),
        model: cfg.agents.defaults.model.clone(),
    };

    // Create LLM-backed decider that calls the provider with tool-use
    let heartbeat_provider = make_provider(&cfg)?;
    let decider: Arc<dyn HeartbeatDecider> = Arc::new(heartbeat::LLMHeartbeatDecider::new(
        heartbeat_provider,
        cfg.agents.defaults.model.clone(),
    ));

    let executor: Arc<dyn HeartbeatExecutor> = if let Some(ref mgr) = channel_mgr {
        Arc::new(AgentExecutor {
            agent: agent.clone(),
            channel_mgr: mgr.clone(),
            keep_recent_messages: cfg.gateway.heartbeat.keep_recent_messages as usize,
        })
    } else {
        Arc::new(AgentExecutor {
            agent: agent.clone(),
            channel_mgr: ChannelManager::new(Arc::new(cfg.clone()), (*bus).clone())
                .map_err(|e| format!("channel manager: {e}"))?,
            keep_recent_messages: cfg.gateway.heartbeat.keep_recent_messages as usize,
        })
    };

    let notifier: Option<Arc<dyn heartbeat::HeartbeatNotifier>> = if let Some(ref mgr) = channel_mgr
    {
        Some(Arc::new(HeartbeatChannelNotifier {
            bus: bus.clone(),
            channel_mgr: mgr.clone(),
        }))
    } else {
        None
    };

    let hb = Arc::new(HeartbeatService::new(
        hb_cfg,
        decider,
        Some(executor),
        notifier,
        None,
    ));
    hb.clone().start().await;

    // ---- /health endpoint ----
    let listener = TcpListener::bind(&bind)
        .await
        .map_err(|e| format!("bind {bind}: {e}"))?;
    let started_at = SystemTime::now();
    let model = cfg.agents.defaults.model.clone();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accept = listener.accept() => {
                    let (mut stream, _) = match accept {
                        Ok(v) => v,
                        Err(e) => {
                            log::warn!("gateway accept error: {e}");
                            continue;
                        }
                    };
                    let model = model.clone();
                    let started_at = started_at;
                    tokio::spawn(async move {
                        let _ = handle_health_request(&mut stream, &model, started_at).await;
                    });
                }
            }
        }
    });

    // ---- agent bus loop ----
    let agent_for_loop = agent.clone();
    let agent_task = tokio::spawn(async move { AgentLoop::run(agent_for_loop).await });

    println!("nanobot gateway listening on http://{bind} (health endpoint)");
    println!("workspace: {}", cfg.workspace_path().display());
    let channel_count = match &channel_mgr {
        Some(mgr) => mgr.enabled_channels().await.len(),
        None => 0,
    };
    println!(
        "cron: enabled  heartbeat: {}  channels: {channel_count}",
        if cfg.gateway.heartbeat.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );

    let _ = signal::ctrl_c().await;
    println!("\nshutting down…");

    if let Some(mgr) = &channel_mgr {
        mgr.stop_all().await;
    }
    let _ = shutdown_tx.send(());
    cron_svc.stop().await;
    hb.stop();
    server.abort();
    agent_task.abort();

    // Flush all cached sessions to durable storage before exit.
    // This prevents data loss on filesystems with write-back
    // caching (rclone VFS, NFS, FUSE mounts, etc.).
    let flushed = agent.flush_sessions();
    if flushed > 0 {
        log::info!("Shutdown: flushed {} session(s) to disk", flushed);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Cron handler — converts a [`CronJob`] firing into an inbound message.
// ---------------------------------------------------------------------------

struct CronAgentHandler {
    agent: Arc<agent::AgentLoop>,
    dream: Arc<MemoryDream>,
}

#[async_trait]
impl JobHandler for CronAgentHandler {
    async fn on_job(&self, job: &CronJob) -> Result<Option<String>, String> {
        // System jobs are dispatched by id rather than rewritten as
        // inbound messages.
        if job.id == "system::dream" {
            let did_work = self.dream.run().await;
            return Ok(Some(if did_work {
                "dream: completed".into()
            } else {
                "dream: nothing to process".into()
            }));
        }

        // Set cron context to mark that we're executing within a cron job
        let cron_tool = self.agent.tools().get("cron").await;
        if let Some(_tool) = cron_tool {
            // The tool is stored as Arc<dyn Tool>, we need to downcast to CronTool
            // For now, we use a simpler approach without context management
        }

        let channel = job
            .payload
            .channel
            .clone()
            .unwrap_or_else(|| "cron".to_string());
        let chat_id = job.payload.to.clone().unwrap_or_else(|| job.id.clone());
        let inbound = InboundMessage {
            channel: channel.clone(),
            sender_id: "cron".into(),
            chat_id: chat_id.clone(),
            content: job.payload.message.clone(),
            timestamp: Local::now(),
            media: Vec::new(),
            metadata: Default::default(),
            session_key_override: None,
        };

        let response = self
            .agent
            .process_inbound(inbound)
            .await
            .map(|r| r.final_content)?;

        // Handle deliver flag - if the job payload requests delivery and we have
        // a response, publish it as an outbound message
        if job.payload.deliver
            && !job
                .payload
                .to
                .as_ref()
                .map(|s| s.is_empty())
                .unwrap_or(true)
        {
            if let Some(ref content) = response {
                if !content.is_empty() {
                    let outbound = bus::OutboundMessage {
                        channel,
                        chat_id,
                        content: content.clone(),
                        reply_to: None,
                        media: Vec::new(),
                        metadata: Default::default(),
                    };
                    self.agent.bus().publish_outbound(outbound).await;
                }
            }
        }

        Ok(response)
    }
}

// ---------------------------------------------------------------------------
// Heartbeat plug-ins.
// ---------------------------------------------------------------------------

/// Pick a routable channel/chat target for heartbeat-triggered messages.
/// Mirrors the Python `_pick_heartbeat_target` logic.
fn pick_heartbeat_target(
    channel_mgr: &Option<Arc<channels::ChannelManager>>,
) -> Option<(String, String)> {
    match channel_mgr {
        Some(mgr) => {
            let enabled_channels = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(mgr.enabled_channels())
            });
            let enabled: std::collections::HashSet<String> = enabled_channels.into_iter().collect();

            for channel in &enabled {
                if channel != "cli" && channel != "system" {
                    return Some((channel.clone(), "default".into()));
                }
            }

            None
        }
        None => None,
    }
}

/// Notifier that delivers heartbeat responses to the user's channel.
struct HeartbeatChannelNotifier {
    bus: Arc<bus::MessageBus>,
    channel_mgr: Arc<channels::ChannelManager>,
}

#[async_trait]
impl heartbeat::HeartbeatNotifier for HeartbeatChannelNotifier {
    async fn notify(&self, text: &str) {
        if let Some((channel, chat_id)) = pick_heartbeat_target(&Some(self.channel_mgr.clone())) {
            if channel == "cli" {
                return; // No external channel available to deliver to
            }
            let outbound = bus::OutboundMessage {
                channel,
                chat_id,
                content: text.to_string(),
                reply_to: None,
                media: Vec::new(),
                metadata: Default::default(),
            };
            self.bus.publish_outbound(outbound).await;
        }
    }
}

struct AgentExecutor {
    agent: Arc<agent::AgentLoop>,
    channel_mgr: Arc<channels::ChannelManager>,
    /// Maximum number of recent messages to retain between heartbeat runs.
    keep_recent_messages: usize,
}

#[async_trait]
impl HeartbeatExecutor for AgentExecutor {
    async fn execute(&self, tasks: &str) -> Option<String> {
        let (channel, chat_id) = pick_heartbeat_target(&Some(self.channel_mgr.clone()))
            .unwrap_or_else(|| ("heartbeat".into(), "heartbeat".into()));

        let inbound = InboundMessage {
            channel,
            sender_id: "heartbeat".into(),
            chat_id,
            content: tasks.to_string(),
            timestamp: Local::now(),
            media: Vec::new(),
            metadata: Default::default(),
            session_key_override: Some("heartbeat:default".into()),
        };

        let result = self
            .agent
            .process_inbound(inbound)
            .await
            .ok()
            .and_then(|r| r.final_content);

        // Keep a small tail of heartbeat history so the loop stays bounded
        // without losing all short-term context between runs.
        if self.keep_recent_messages > 0 {
            self.agent
                .retain_heartbeat_session(self.keep_recent_messages);
        }

        result
    }
}

// ---------------------------------------------------------------------------
// /health handler — minimal raw-HTTP responder. Avoids pulling axum into
// the cli crate just for one route.
// ---------------------------------------------------------------------------

async fn handle_health_request(
    stream: &mut tokio::net::TcpStream,
    model: &str,
    started_at: SystemTime,
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 1024];
    let _ = stream.read(&mut buf).await?;

    let uptime_s = SystemTime::now()
        .duration_since(started_at)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = format!("{{\"status\":\"ok\",\"model\":\"{model}\",\"uptime_s\":{uptime_s}}}");
    let resp = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len(),
        body = body,
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.shutdown().await
}

// ---------------------------------------------------------------------------
// Dream system job registration. Mirrors the Python "Dream" registration
// — a recurring `every Nh` system event that the cron service surfaces
// to [`CronAgentHandler`], which dispatches by `job.id == "system::dream"`
// and runs [`MemoryDream::run`] for actual two-phase memory consolidation.
// ---------------------------------------------------------------------------

async fn register_dream_job(svc: &Arc<::cron::CronService>, cfg: &config::Config) {
    let interval_h = cfg.agents.defaults.dream.interval_h.max(1);
    let interval_ms: i64 = (interval_h as i64).saturating_mul(3_600_000);
    let job = CronJob {
        id: "system::dream".into(),
        name: "Dream".into(),
        enabled: true,
        schedule: CronSchedule {
            kind: ScheduleKind::Every,
            every_ms: Some(interval_ms),
            ..Default::default()
        },
        payload: CronPayload {
            kind: PayloadKind::SystemEvent,
            message: "[dream]".into(),
            deliver: false,
            channel: None,
            to: None,
            channel_meta: None,
            session_key: None,
        },
        state: Default::default(),
        created_at_ms: 0,
        updated_at_ms: 0,
        delete_after_run: false,
    };
    svc.register_system_job(job).await;
}
