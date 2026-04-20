use llm_relay_core::ipc::codec::{read_frame, write_frame};
use llm_relay_core::ipc::protocol::*;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

#[tokio::test]
async fn round_trip_request_and_event() {
    let (mut a, mut b) = duplex(8192);

    let req = ClientFrame {
        request_id: 42,
        payload: Request::SetActive {
            gateway_id: Uuid::new_v4(),
            key_id: Uuid::new_v4(),
            models: ModelSelection { claude: Some("sonnet".into()), ..Default::default() },
        },
    };
    write_frame(&mut a, &req).await.unwrap();
    a.shutdown().await.ok();

    let got: ClientFrame = read_frame(&mut b).await.unwrap();
    assert_eq!(got.request_id, 42);
    match got.payload {
        Request::SetActive { models, .. } => assert_eq!(models.claude.as_deref(), Some("sonnet")),
        _ => panic!("wrong variant"),
    }
}

#[tokio::test]
async fn server_frame_event_kind_intact() {
    let (mut a, mut b) = duplex(4096);
    let frame = ServerFrame::Event(Event::HealthChanged {
        gateway_id: Uuid::new_v4(),
        status: HealthStatus::Healthy,
    });
    write_frame(&mut a, &frame).await.unwrap();
    a.shutdown().await.ok();

    let got: ServerFrame = read_frame(&mut b).await.unwrap();
    match got {
        ServerFrame::Event(Event::HealthChanged { status, .. }) => {
            assert_eq!(status, HealthStatus::Healthy);
        }
        _ => panic!("wrong frame"),
    }
}

#[tokio::test]
async fn server_frame_response_carries_request_id() {
    let (mut a, mut b) = duplex(4096);
    let frame = ServerFrame::Response {
        request_id: 7,
        payload: Response::LoginInitiated {
            gateway_id: Uuid::new_v4(),
            user_code: "ABCD-1234".into(),
            verification_uri: "https://gw/device/login".into(),
            expires_in_secs: 600,
        },
    };
    write_frame(&mut a, &frame).await.unwrap();
    a.shutdown().await.ok();

    let got: ServerFrame = read_frame(&mut b).await.unwrap();
    match got {
        ServerFrame::Response { request_id, payload: Response::LoginInitiated { user_code, .. } } => {
            assert_eq!(request_id, 7);
            assert_eq!(user_code, "ABCD-1234");
        }
        _ => panic!("wrong frame"),
    }
}

#[tokio::test]
async fn rejects_oversize_frame() {
    let (mut a, mut b) = duplex(64);
    // Fake frame: claim 100 MB length
    a.write_all(&100_000_000u32.to_be_bytes()).await.unwrap();
    a.shutdown().await.ok();

    let res: std::io::Result<ClientFrame> = read_frame(&mut b).await;
    assert!(res.is_err());
}
