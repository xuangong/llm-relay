use llm_relay_agent::{ipc_server, lifecycle, login};
use anyhow::Result;
use chrono::Utc;
use llm_relay_core::{paths, Database, Service};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    init_log();
    let _guard = lifecycle::LifecycleGuard::acquire()?;
    log::info!("agent starting (pid {})", std::process::id());

    std::fs::create_dir_all(paths::config_dir())?;
    std::fs::create_dir_all(paths::runtime_dir())?;
    llm_relay_core::keystore::init(&paths::config_dir());

    let db = Arc::new(Database::init(&paths::config_dir())?);
    let bus = ipc_server::EventBus::new();
    let sink: llm_relay_core::SharedEventSink = Arc::new(ipc_server::BusSink { bus: bus.clone() });
    let service = Service::new(db.clone(), sink);

    // Spawn proxy + health
    let s1 = service.clone();
    tokio::spawn(async move { llm_relay_core::proxy_server::start(s1).await });
    let s2 = service.clone();
    tokio::spawn(async move { llm_relay_core::health::health_check_loop(s2).await });

    let login_registry = Arc::new(login::LoginRegistry::new(bus.0.clone()));
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let ctx = ipc_server::ServerCtx {
        service,
        bus,
        agent_started_at: Utc::now(),
        agent_pid: std::process::id(),
        keystore_kind: llm_relay_core::keystore::current_kind(),
        shutdown: shutdown.clone(),
        login_registry,
    };

    // Listen for SIGTERM / Ctrl-C and trip shutdown.
    let sd = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        log::info!("ctrl_c received");
        sd.notify_one();
    });

    ipc_server::run(&paths::sock_file(), ctx).await?;
    log::info!("agent exiting cleanly");
    Ok(())
}

fn init_log() {
    // Append-only log file at ~/.llm-relay/agent.log.
    use std::io::Write as _;
    let path = paths::log_file();
    let _ = std::fs::create_dir_all(paths::config_dir());
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let target = Box::new(file);
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .target(env_logger::Target::Pipe(target))
            .format(|buf, rec| writeln!(buf, "[{}] {} {}: {}", chrono::Utc::now().to_rfc3339(), rec.level(), rec.target(), rec.args()))
            .init();
    } else {
        env_logger::init();
    }
}
