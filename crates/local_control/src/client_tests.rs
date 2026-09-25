#[cfg(unix)]
use std::io::{Read as _, Write as _};

#[cfg(unix)]
use chrono::Duration;
use chrono::Utc;
use uuid::Uuid;

use super::*;
#[cfg(unix)]
use crate::auth::CredentialGrant;
use crate::discovery::{ControlEndpoint, CredentialBrokerReference, InstanceId};
#[cfg(unix)]
#[test]
fn credential_client_exchanges_request_over_broker_socket() {
    let dir = tempfile::tempdir().expect("temp dir");
    let socket_path = dir.path().join("broker.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket_path).expect("broker binds");
    let grant = CredentialGrant::new(
        InstanceId("inst_expected".to_owned()),
        ActionKind::AppPing,
        Duration::minutes(5),
    );
    let credential = ScopedCredential {
        bearer_token: "scoped-token".to_owned(),
        grant,
    };
    let expected_request = CredentialRequest::new(ActionKind::AppPing);
    let server_request = expected_request.clone();
    let server_credential = credential.clone();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("broker accepts");
        let mut bytes = Vec::new();
        stream
            .read_to_end(&mut bytes)
            .expect("broker reads request");
        let request = serde_json::from_slice::<CredentialRequest>(&bytes).expect("request decodes");
        assert_eq!(request, server_request);
        serde_json::to_writer(&mut stream, &server_credential).expect("broker writes credential");
        stream.flush().expect("broker flushes credential");
    });

    let response = request_credential_over_socket(&socket_path, &expected_request)
        .expect("credential exchange succeeds");
    server.join().expect("broker server completes");
    assert_eq!(
        serde_json::from_str::<ScopedCredential>(&response).expect("response decodes"),
        credential
    );
}

#[test]
fn probe_rejects_mismatched_instance_identity() {
    let instance = InstanceRecord {
        protocol_version: crate::PROTOCOL_VERSION,
        instance_id: InstanceId("inst_expected".to_owned()),
        pid: std::process::id(),
        channel: "local".to_owned(),
        app_id: "dev.warp.WarpLocal".to_owned(),
        app_version: None,
        started_at: Utc::now(),
        executable_path: None,
        endpoint: Some(ControlEndpoint::localhost(4000)),
        credential_broker: Some(CredentialBrokerReference {
            socket_path: "inst_expected.broker.sock".into(),
        }),
        actions: vec![ActionKind::AppPing.metadata()],
    };
    let err = validate_probe_response(
        &instance,
        ResponseEnvelope::ok(
            Uuid::new_v4(),
            serde_json::json!({ "instance_id": "inst_other" }),
        ),
    )
    .expect_err("mismatched live identity is rejected");
    assert_eq!(err.code, ErrorCode::TransportUnavailable);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn post_request_gives_up_when_the_app_does_not_answer_in_time() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener binds");
    let url = format!("http://{}/v1/control", listener.local_addr().expect("local addr"));
    let request = RequestEnvelope::new(Action::new(ActionKind::AppPing));

    let err = post_request(&url, "Bearer token", &request, std::time::Duration::from_millis(200))
        .expect_err("silent server times out");

    assert_eq!(err.code, ErrorCode::TransportUnavailable);
    drop(listener);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn post_request_waits_longer_than_a_short_timeout_when_allowed() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener binds");
    let url = format!("http://{}/v1/control", listener.local_addr().expect("local addr"));
    let request = RequestEnvelope::new(Action::new(ActionKind::AppPing));
    let response = ResponseEnvelope::ok(request.request_id, serde_json::json!({ "ok": true }));
    let body = serde_json::to_string(&response).expect("response serializes");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("client connects");
        let mut buffer = [0u8; 4096];
        let read = stream.read(&mut buffer).expect("request is readable");
        assert!(read > 0);
        std::thread::sleep(std::time::Duration::from_millis(400));
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("response is writable");
    });

    let envelope = post_request(&url, "Bearer token", &request, std::time::Duration::from_secs(10))
        .expect("slow server still answers within the timeout");
    server.join().expect("server completes");

    assert_eq!(envelope.request_id, request.request_id);
}
