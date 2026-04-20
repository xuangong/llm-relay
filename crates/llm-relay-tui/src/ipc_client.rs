//! IPC client used by the TUI. Owns one connection to the agent.
//!
//! Architecture:
//! - A reader task pulls `ServerFrame`s off the socket, routes `Response`s to
//!   the matching pending oneshot, and broadcasts `Event`s to subscribers.
//! - A writer mutex serializes `ClientFrame` writes.
//! - `request(payload)` allocates a `request_id`, registers a oneshot,
//!   writes the frame, awaits the oneshot. Times out after 30s.

use llm_relay_core::ipc::{ClientFrame, Event, Request, Response, ServerFrame};
use llm_relay_core::ipc::codec::{read_frame, write_frame};
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, oneshot, Mutex};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("connection closed")]
    Closed,
    #[error("request timed out")]
    Timeout,
    #[error("agent error: {0}")]
    Agent(String),
}

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>;

#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(windows)]
type Stream = interprocess::os::windows::named_pipe::tokio::DuplexPipeStream<
    interprocess::os::windows::named_pipe::pipe_mode::Bytes,
>;

pub struct IpcClient {
    writer: Arc<Mutex<tokio::io::WriteHalf<Stream>>>,
    pending: PendingMap,
    events_tx: broadcast::Sender<Event>,
}

impl IpcClient {
    pub async fn connect(socket: &Path) -> Result<Arc<Self>, ClientError> {
        #[cfg(unix)]
        let stream = tokio::net::UnixStream::connect(socket).await?;
        #[cfg(windows)]
        let stream = {
            let s = socket.to_string_lossy().to_string();
            interprocess::os::windows::named_pipe::tokio::DuplexPipeStream::<
                interprocess::os::windows::named_pipe::pipe_mode::Bytes,
            >::connect(s).await?
        };

        let (read_half, write_half) = tokio::io::split(stream);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, _) = broadcast::channel(256);

        let client = Arc::new(Self {
            writer: Arc::new(Mutex::new(write_half)),
            pending: pending.clone(),
            events_tx: events_tx.clone(),
        });

        tokio::spawn(reader_loop(read_half, pending, events_tx));

        Ok(client)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }

    pub async fn request(
        &self,
        payload: Request,
    ) -> Result<Response, ClientError> {
        let (tx, rx) = oneshot::channel();
        let request_id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().await.insert(request_id, tx);

        let frame = ClientFrame { request_id, payload };
        {
            let mut w = self.writer.lock().await;
            write_frame(&mut *w, &frame).await?;
        }

        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(resp)) => match resp {
                Response::Error { message } => Err(ClientError::Agent(message)),
                other => Ok(other),
            },
            Ok(Err(_)) => Err(ClientError::Closed),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err(ClientError::Timeout)
            }
        }
    }
}

async fn reader_loop(
    mut read: tokio::io::ReadHalf<Stream>,
    pending: PendingMap,
    events: broadcast::Sender<Event>,
) {
    loop {
        match read_frame::<_, ServerFrame>(&mut read).await {
            Ok(ServerFrame::Response { request_id, payload }) => {
                if let Some(tx) = pending.lock().await.remove(&request_id) {
                    let _ = tx.send(payload);
                }
            }
            Ok(ServerFrame::Event(evt)) => {
                let _ = events.send(evt);
            }
            Err(_) => {
                // Drain pending with Closed errors via dropping senders.
                pending.lock().await.clear();
                break;
            }
        }
    }
}
