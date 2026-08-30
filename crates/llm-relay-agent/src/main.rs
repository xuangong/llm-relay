use llm_relay_agent::{ipc_server, lifecycle, login};
use anyhow::Result;
use chrono::Utc;
use llm_relay_core::{paths, Database, Service};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    init_log();
    // LifecycleGuard binds port 18080 atomically with the file lock — no separate
    // probe step (which would race a competing process between probe and proxy bind).
    let mut guard = lifecycle::LifecycleGuard::acquire().map_err(|e| {
        eprintln!("{e}");
        anyhow::anyhow!("{e}")
    })?;
    log::info!("agent starting (pid {})", std::process::id());

    std::fs::create_dir_all(paths::config_dir())?;
    std::fs::create_dir_all(paths::runtime_dir())?;
    // Headless agent uses env-only keystore: master key from
    // LLM_RELAY_MASTER_KEY, ciphertext at ~/.llm-relay/secrets.env.enc.
    // No OS keychain, no interactive prompt — the agent is meant for
    // server / TUI deployments where neither makes sense.
    if let Err(e) = llm_relay_core::keystore::init_env(&paths::config_dir()) {
        use llm_relay_core::keystore::EnvInitError;
        eprintln!("error: {e}");
        // The "generate a key" hint only helps someone who has no key. For a
        // key that simply doesn't match the store, it would send them off to
        // create a second wrong one.
        if matches!(e, EnvInitError::MissingKey(_)) {
            eprintln!("\n{}", llm_relay_core::keystore::env_setup_hint());
        }
        std::process::exit(2);
    }

    let db = Arc::new(Database::init(&paths::config_dir())?);
    let bus = ipc_server::EventBus::new();
    let sink: llm_relay_core::SharedEventSink = Arc::new(ipc_server::BusSink { bus: bus.clone() });
    let service = Service::new(db.clone(), sink);

    // Spawn proxy + health. Hand off the pre-bound listeners so we don't
    // re-bind and risk a TOCTOU race against another process. The WSL
    // listener (if present) gets a serve task too, sharing the same
    // ProxyState.
    let primary = guard.take_listener().expect("primary listener pre-bound by lifecycle");
    let initial_wsl = guard.wsl_listener.take();
    let service_arc = Arc::new(service.clone());
    let proxy_state = llm_relay_core::proxy_server::ProxyState::new(
        service.db.clone(),
        service.switch_lock.clone(),
        service.sink.clone(),
    )
    .with_service(service_arc);
    let proxy_handle = llm_relay_core::proxy_server::start_with_listeners(
        proxy_state,
        primary,
        initial_wsl,
    )
    .await;
    let service = service.with_proxy(proxy_handle.clone());
    let s2 = service.clone();
    tokio::spawn(async move { llm_relay_core::health::health_check_loop(s2).await });

    // Start the WSL detection state machine. On non-Windows / no-WSL
    // hosts the first tick finds nothing and the machine settles into
    // Lazy mode (no periodic work).
    let _wsl_sm = service.spawn_wsl_state_machine().map(|sm| {
        let sm_run = sm.clone();
        tokio::spawn(async move { sm_run.run().await; });
        sm
    });

    let login_registry = Arc::new(login::LoginRegistry::new(bus.0.clone()));
    let shutdown = Arc::new(tokio::sync::Notify::new());

    // Persist session_token when login completes.
    {
        let service = service.clone();
        let mut rx = bus.subscribe();
        tokio::spawn(async move {
            use llm_relay_core::ipc::Event;
            loop {
                match rx.recv().await {
                    Ok(Event::LoginCompleted { gateway_id, session_token, user_id, user_name }) => {
                        log::info!("persisting session token for gateway {gateway_id}");
                        if let Err(e) = service.save_login_session(gateway_id, session_token, user_id, user_name).await {
                            log::error!("failed to save login session: {e}");
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("login listener lagged {n}");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    let ctx = ipc_server::ServerCtx {
        service,
        bus,
        agent_started_at: Utc::now(),
        agent_pid: std::process::id(),
        keystore_kind: llm_relay_core::keystore::current_kind(),
        shutdown: shutdown.clone(),
        login_registry,
    };

    // Listen for SIGTERM / Ctrl-C and trip shutdown so Drop runs and
    // pid/sock files get cleaned up.
    let sd = shutdown.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
            let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
            tokio::select! {
                _ = sigterm.recv() => log::info!("SIGTERM received"),
                _ = sigint.recv() => log::info!("SIGINT received"),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            log::info!("ctrl_c received");
        }
        sd.notify_one();
    });

    ipc_server::run(&paths::sock_file(), ctx).await?;
    log::info!("agent exiting cleanly");
    // Run guard's Drop now (cleans pid/sock + releases lock) instead of
    // letting it happen during runtime drop, which can hang waiting on
    // background tasks (proxy, health) that have no shutdown signal.
    drop(guard);
    // Force-terminate. The proxy and health-check loops are infinite by
    // design and will block runtime drop forever; calling exit() is the
    // simplest correct shutdown for a process whose only job is to vanish.
    std::process::exit(0);
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
