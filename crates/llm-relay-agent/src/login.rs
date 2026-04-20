//! Per-gateway login state machine.
//!
//! `LoginRegistry::start` kicks off a device-code flow against a gateway,
//! returns the `DeviceCodeResponse` so the IPC layer can answer `LoginInitiated`,
//! and spawns a background poller that emits `LoginCompleted | LoginFailed |
//! LoginExpired` events on the broadcast bus. Concurrent `start` calls for the
//! same gateway are rejected.

use llm_relay_core::gateway::{self, DeviceCodeResponse};
use llm_relay_core::ipc::Event;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug)]
pub enum LoginOutcome {
    Completed {
        session_token: String,
        user_id: Option<String>,
        user_name: Option<String>,
    },
    Failed(String),
    Expired,
}

struct Session {
    handle: JoinHandle<()>,
}

#[derive(Clone)]
pub struct LoginRegistry {
    inner: Arc<Mutex<HashMap<Uuid, Session>>>,
    events: broadcast::Sender<Event>,
}

impl LoginRegistry {
    pub fn new(events: broadcast::Sender<Event>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            events,
        }
    }

    /// Start a real device-code login against `url`. Returns the device-code
    /// payload so the caller can answer `LoginInitiated` to the requesting
    /// client. Spawns a background poller; emits final event on the bus.
    pub async fn start(
        &self,
        gateway_id: Uuid,
        url: String,
    ) -> Result<DeviceCodeResponse, StartError> {
        // Fast guard: don't even hit the network if a session is already live.
        {
            let map = self.inner.lock().await;
            if map.contains_key(&gateway_id) {
                return Err(StartError::AlreadyRunning);
            }
        }
        let code = gateway::request_device_code(&url)
            .await
            .map_err(|e| StartError::Network(e.to_string()))?;

        let device_code = code.device_code.clone();
        let interval = Duration::from_secs(code.interval.max(1));
        let expires = Duration::from_secs(code.expires_in);
        let events = self.events.clone();
        let url_for_task = url.clone();
        let inner = self.inner.clone();

        let poller = async move {
            poll_until_done(&url_for_task, &device_code, interval, expires).await
        };
        let handle = tokio::spawn(async move {
            let outcome = poller.await;
            let evt = outcome_to_event(gateway_id, outcome);
            // Drop registry entry first so subsequent start calls are allowed.
            let _ = inner.lock().await.remove(&gateway_id);
            let _ = events.send(evt);
        });

        self.inner
            .lock()
            .await
            .insert(gateway_id, Session { handle });

        Ok(code)
    }

    /// Test-only / DI seam: start a login with a hand-crafted poller future.
    /// Returns Some(()) if started, None if a session was already in flight.
    pub async fn start_with_poller<F>(
        &self,
        gateway_id: Uuid,
        poller: F,
    ) -> Option<()>
    where
        F: Future<Output = LoginOutcome> + Send + 'static,
    {
        {
            let map = self.inner.lock().await;
            if map.contains_key(&gateway_id) {
                return None;
            }
        }
        let events = self.events.clone();
        let inner = self.inner.clone();
        let handle = tokio::spawn(async move {
            let outcome = poller.await;
            let evt = outcome_to_event(gateway_id, outcome);
            let _ = inner.lock().await.remove(&gateway_id);
            let _ = events.send(evt);
        });
        self.inner
            .lock()
            .await
            .insert(gateway_id, Session { handle });
        Some(())
    }

    /// Cancel a running login. Returns true if a session was in flight.
    pub async fn cancel(&self, gateway_id: Uuid) -> bool {
        if let Some(session) = self.inner.lock().await.remove(&gateway_id) {
            session.handle.abort();
            true
        } else {
            false
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("a login is already in progress for this gateway")]
    AlreadyRunning,
    #[error("network error: {0}")]
    Network(String),
}

async fn poll_until_done(
    url: &str,
    device_code: &str,
    interval: Duration,
    expires: Duration,
) -> LoginOutcome {
    let deadline = Instant::now() + expires;
    loop {
        if Instant::now() >= deadline {
            return LoginOutcome::Expired;
        }
        tokio::time::sleep(interval).await;
        match gateway::poll_device_code(url, device_code).await {
            Ok(resp) => match resp.status.as_str() {
                "approved" | "completed" | "ok" => {
                    return LoginOutcome::Completed {
                        session_token: resp.session_token.unwrap_or_default(),
                        user_id: resp.user_id,
                        user_name: resp.user_name,
                    };
                }
                "pending" | "authorization_pending" | "slow_down" => continue,
                "denied" | "access_denied" => {
                    return LoginOutcome::Failed("access_denied".into());
                }
                "expired" | "expired_token" => return LoginOutcome::Expired,
                other => return LoginOutcome::Failed(format!("unknown status: {other}")),
            },
            Err(e) => return LoginOutcome::Failed(e.to_string()),
        }
    }
}

fn outcome_to_event(gateway_id: Uuid, outcome: LoginOutcome) -> Event {
    match outcome {
        LoginOutcome::Completed { session_token, user_id, user_name } => {
            Event::LoginCompleted { gateway_id, session_token, user_id, user_name }
        }
        LoginOutcome::Failed(message) => Event::LoginFailed { gateway_id, message },
        LoginOutcome::Expired => Event::LoginExpired { gateway_id },
    }
}
