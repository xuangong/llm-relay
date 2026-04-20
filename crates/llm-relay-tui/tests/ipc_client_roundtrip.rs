use llm_relay_core::ipc::{ClientFrame, Event, HealthStatus, Request, Response, ServerFrame};
use llm_relay_core::ipc::codec::{read_frame, write_frame};
use llm_relay_tui::ipc_client::IpcClient;
use tokio::net::UnixListener;
use uuid::Uuid;

#[tokio::test]
async fn ping_returns_pong_via_request_id() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("agent.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    // Fake agent: one accept, echo Ping → Pong with the right request_id
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let frame: ClientFrame = read_frame(&mut stream).await.unwrap();
        assert!(matches!(frame.payload, Request::Ping));
        let resp = ServerFrame::Response {
            request_id: frame.request_id,
            payload: Response::Pong,
        };
        write_frame(&mut stream, &resp).await.unwrap();
        // Hold the connection so the client doesn't see EOF before reading.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });

    let client = IpcClient::connect(&sock).await.unwrap();
    let resp = client.request(Request::Ping).await.unwrap();
    assert!(matches!(resp, Response::Pong));
    server.await.unwrap();
}

#[tokio::test]
async fn events_are_broadcast_to_subscribers() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("agent.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        // Push a HealthChanged event spontaneously
        let evt = ServerFrame::Event(Event::HealthChanged {
            gateway_id: Uuid::nil(),
            status: HealthStatus::Healthy,
        });
        write_frame(&mut stream, &evt).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });

    let client = IpcClient::connect(&sock).await.unwrap();
    let mut sub = client.subscribe();
    let evt = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
        .await.unwrap().unwrap();
    match evt {
        Event::HealthChanged { status, .. } => assert_eq!(status, HealthStatus::Healthy),
        other => panic!("unexpected event: {other:?}"),
    }
    server.await.unwrap();
}
