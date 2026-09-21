//! Regression checks for the supplied b29dd98 archive.
//! Prepared by source inspection. NOT compiled/run by the reviewer.
//! Calls the real public Notifier against loopback HTTP with dummy credentials.
//! Checks independently decoded payload, not merely encoded length or X counts.
use jev_sentinel::actions::notifier::{AlertReason, AlertSeverity, AlertSource, Notifier};
use jev_sentinel::config::TelegramAlertSettings;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

struct Mock {
    base: String,
    seen: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn request_body(socket: &mut TcpStream) -> Option<Value> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let (end, length) = loop {
        let n = socket.read(&mut buffer).await.ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..n]);
        if bytes.len() > 128 * 1024 {
            return None;
        }
        if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let length = std::str::from_utf8(&bytes[..pos])
                .ok()?
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))?
                .1
                .trim()
                .parse::<usize>()
                .ok()?;
            if length > 128 * 1024 {
                return None;
            }
            break (pos + 4, length);
        }
    };
    while bytes.len() < end + length {
        let n = socket.read(&mut buffer).await.ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    serde_json::from_slice(&bytes[end..end + length]).ok()
}

impl Mock {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let body =
                    match tokio::time::timeout(Duration::from_secs(5), request_body(&mut socket))
                        .await
                    {
                        Ok(Some(body)) => body,
                        _ => continue,
                    };
                records.lock().unwrap().push(body);
                let payload = r#"{"ok":true}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    payload.len(), payload
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        Self { base, seen, task }
    }
}

// Minimal decoder for exactly the fixture's tags and four named entities.
// Mirrors the named-entity branch of TDLib decode_html_entity / parse_html:
// known names decode (semicolon is optional); unknown fragments stay literal.
// This is not a full Telegram mock/parser; no numeric entities/attributes are used.
// Reference: https://raw.githubusercontent.com/tdlib/td/master/td/telegram/MessageEntity.cpp
fn render_fixture_html(text: &str) -> String {
    let mut output = String::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let end = text[i..].find('>').expect("fixture tag must close");
            let tag = &text[i..i + end + 1];
            assert!(matches!(
                tag,
                "<b>" | "</b>" | "<code>" | "</code>" | "<i>" | "</i>"
            ));
            i += end + 1;
            continue;
        }
        if bytes[i] == b'&' {
            let mut end = i + 1;
            while end < bytes.len() && bytes[end].is_ascii_alphabetic() {
                end += 1;
            }
            let replacement = match &text[i + 1..end] {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "quot" => Some('"'),
                _ => None,
            };
            if let Some(c) = replacement {
                output.push(c);
                i = end + usize::from(bytes.get(end) == Some(&b';'));
                continue;
            }
        }
        let c = text[i..].chars().next().unwrap();
        output.push(c);
        i += c.len_utf8();
    }
    output
}

async fn assert_roundtrip(payload: &str) {
    let mock = Mock::start().await;
    let n = Notifier::new_with_api_base(
        Some(TelegramAlertSettings {
            bot_token: "review-dummy-token".into(),
            chat_id: "0".into(),
            min_severity: "info".into(),
            proxy: None,
        }),
        &mock.base,
    );
    let event = n.build_alert_event(
        AlertSource::LocalRule,
        Some("node-a".into()),
        AlertSeverity::Warning,
        AlertReason::ServiceCheckFailed,
        "Review",
        payload,
    );
    tokio::time::timeout(Duration::from_secs(5), n.dispatch_alert_event(&event))
        .await
        .expect("bounded fixture exchange")
        .expect("dispatch result");
    let mut visible = String::new();
    for request in mock.seen.lock().unwrap().iter() {
        let raw = request["text"].as_str().unwrap();
        let decoded = if request["parse_mode"].as_str() == Some("HTML") {
            render_fixture_html(raw)
        } else {
            raw.to_owned()
        };
        assert!(decoded.chars().count() <= 4096, "visible message length");
        visible.push_str(&decoded);
    }
    let begin = visible.find("PAYLOAD|").expect("fixture prefix");
    let end = visible.rfind("|END").expect("fixture suffix") + "|END".len();
    assert_eq!(
        &visible[begin..end],
        payload,
        "independently rendered messages must preserve the original logical text"
    );
}

#[tokio::test]
async fn control_long_plain_ascii_keeps_all_text() {
    assert_roundtrip(&format!("PAYLOAD|{}|END", "X".repeat(5000))).await;
}

#[tokio::test]
async fn regression_quoted_payload_must_survive_entity_boundary_split() {
    assert_roundtrip(&format!("PAYLOAD|{}|END", "\"".repeat(1000))).await;
}

#[tokio::test]
async fn regression_ampersand_payload_must_survive_entity_boundary_split() {
    assert_roundtrip(&format!("PAYLOAD|{}|END", "&".repeat(1000))).await;
}
