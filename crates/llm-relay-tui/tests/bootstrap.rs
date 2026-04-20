use llm_relay_tui::bootstrap::{ensure_agent, EnsureMode};

#[tokio::test]
async fn returns_attached_when_socket_already_exists_and_responds() {
    // Set up a fake agent listening on a temp socket.
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("agent.sock");
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    let server = tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            use llm_relay_core::ipc::*;
            use llm_relay_core::ipc::codec::*;
            let frame: ClientFrame = read_frame(&mut stream).await.unwrap();
            let resp = ServerFrame::Response {
                request_id: frame.request_id,
                payload: Response::Pong,
            };
            write_frame(&mut stream, &resp).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });

    let result = ensure_agent(&sock, EnsureMode::AttachOnly).await.unwrap();
    assert!(matches!(result, llm_relay_tui::bootstrap::AgentHandle::Attached(_)));
    server.await.unwrap();
}

#[tokio::test]
async fn fails_attach_only_when_no_socket() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("nope.sock");
    let result = ensure_agent(&sock, EnsureMode::AttachOnly).await;
    assert!(result.is_err());
}
