//! HTTP contract tests use loopback only and never load real credentials.
use jev_sentinel::{InfrastructureSnapshot, JevClient, SentinelError};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn mock(body: &'static str, status: u16) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 2048];
        loop {
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&chunk[..n]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (k, v) = line.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    let payload: serde_json::Value =
                        serde_json::from_slice(&request[end + 4..]).unwrap();
                    assert!(payload["questions"].is_object());
                    break;
                }
            }
        }
        let response = format!("HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, body.len(), body);
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
    });
    (url, handle)
}

async fn evaluate(
    body: &'static str,
    status: u16,
) -> jev_sentinel::Result<jev_sentinel::SentinelDecision> {
    let (url, server) = mock(body, status).await;
    // An explicit loopback proxy avoids inheriting any external environment proxy.
    // The mock accepts absolute-form requests as well as normal origin-form requests.
    let client = JevClient::new(
        "fixture-key".into(),
        None,
        Some(url.clone()),
        Some(url),
        Some(2),
    )
    .unwrap();
    let snapshot = InfrastructureSnapshot {
        timestamp: chrono::Utc::now(),
        targets: vec![],
    };
    let result = tokio::time::timeout(Duration::from_secs(3), client.evaluate_snapshot(&snapshot))
        .await
        .unwrap();
    server.await.unwrap();
    result
}

#[tokio::test]
async fn valid_zero_score_survives_http_without_fabricated_confidence() {
    let result = evaluate(r#"{"model":"test","answers":{"system_health":{"type":"choice","choice":"healthy"},"risk_score":{"type":"score","score":0},"action_required":{"type":"noul","noul":0},"suggested_action":{"type":"choice","choice":"none"}}}"#,200).await.unwrap();
    assert_eq!(result.risk_score, 0.0);
    assert_eq!(result.health_confidence, None);
}

#[tokio::test]
async fn incomplete_or_wrong_type_http_responses_are_invalid_not_healthy() {
    for response in [
        r#"{"model":"test","answers":{}}"#,
        r#"{"model":"test","answers":{"system_health":{"type":"choice","choice":"healthy"},"action_required":{"type":"noul","noul":0},"suggested_action":{"type":"choice","choice":"none"}}}"#,
        r#"{"model":"test","answers":{"risk_score":{"type":"score","score":"0.1"}}}"#,
        "not-json",
    ] {
        assert!(matches!(
            evaluate(response, 200).await,
            Err(SentinelError::InvalidJevResponse(_))
        ));
    }
}

#[tokio::test]
async fn provider_error_body_is_not_reflected() {
    let err = evaluate("do-not-publish-this-provider-secret", 500)
        .await
        .unwrap_err();
    assert!(!err.to_string().contains("do-not-publish"));
    assert!(matches!(err, SentinelError::JevApi(_)));
}
