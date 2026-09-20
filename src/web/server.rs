use crate::core::JevClient;
use crate::error::Result;
use crate::web::models::{default_categories, CategoryDefinition, PublishedStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, RwLock, Semaphore};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

const DASHBOARD_HTML: &str = include_str!("dashboard.html");
const DASHBOARD_CSS: &str = include_str!("dashboard.css");
const DASHBOARD_JS: &str = include_str!("dashboard.js");
const I18N_JS: &str = concat!(
    "globalThis.SentinelEnglish = ",
    include_str!("locales/en.json"),
    ";\n",
    include_str!("i18n.js")
);

const LOCALE_EN: &str = include_str!("locales/en.json");
const LOCALE_RU: &str = include_str!("locales/ru.json");
const LOCALE_DE: &str = include_str!("locales/de.json");
const LOCALE_FR: &str = include_str!("locales/fr.json");
const LOCALE_ES: &str = include_str!("locales/es.json");
const LOCALE_PT_BR: &str = include_str!("locales/pt-BR.json");
const LOCALE_ZH_CN: &str = include_str!("locales/zh-CN.json");
const LOCALE_JA: &str = include_str!("locales/ja.json");
const MAX_HEADER_SIZE: usize = 8192; // 8 KiB
const MAX_BODY_SIZE: usize = 8192; // 8 KiB
const MAX_CONCURRENT_CONNECTIONS: usize = 16;
const READ_TIMEOUT: Duration = Duration::from_secs(3);
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct DynamicWebState {
    pub custom_api_key: Option<String>,
    pub categories: Vec<CategoryDefinition>,
    pub category_map: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeyStatusResponse {
    pub has_custom_key: bool,
    pub active_key_masked: String,
    pub default_key_masked: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SetKeyRequest {
    pub api_key: Option<String>,
}

pub fn mask_key(key: &str) -> String {
    if key.is_ascii() && key.len() > 14 {
        format!("{}...{}", &key[..8], &key[key.len() - 4..])
    } else if !key.is_empty() {
        "***".to_string()
    } else {
        "none".to_string()
    }
}

pub async fn run_web_server(
    listen_addr: String,
    status_rx: watch::Receiver<Arc<PublishedStatus>>,
    status_tx: watch::Sender<Arc<PublishedStatus>>,
    jev_client: Arc<JevClient>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let listener = match TcpListener::bind(&listen_addr).await {
        Ok(l) => {
            info!("Web dashboard listening on http://{}", listen_addr);
            l
        }
        Err(e) => {
            error!("Failed to bind web dashboard to '{}': {}", listen_addr, e);
            return Err(e.into());
        }
    };

    run_web_server_with_listener(listener, status_rx, status_tx, jev_client, shutdown_rx).await
}

pub async fn run_web_server_with_listener(
    listener: TcpListener,
    status_rx: watch::Receiver<Arc<PublishedStatus>>,
    status_tx: watch::Sender<Arc<PublishedStatus>>,
    jev_client: Arc<JevClient>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    let state = Arc::new(RwLock::new(DynamicWebState {
        custom_api_key: None,
        categories: default_categories(),
        category_map: HashMap::new(),
    }));

    loop {
        tokio::select! {
            res = shutdown_rx.changed() => {
                if res.is_err() || *shutdown_rx.borrow() {
                    info!("Web dashboard server shutting down");
                    break;
                }
            }
            accept_res = listener.accept() => {
                match accept_res {
                    Ok((mut stream, peer_addr)) => {
                        let permit = match semaphore.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => {
                                warn!("Web dashboard connection limit reached (max: {}), rejecting from {}", MAX_CONCURRENT_CONNECTIONS, peer_addr);
                                tokio::spawn(async move {
                                    let resp = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 19\r\nConnection: close\r\n\r\nService Unavailable";
                                    let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
                                });
                                continue;
                            }
                        };

                        let rx_clone = status_rx.clone();
                        let tx_clone = status_tx.clone();
                        let jev_clone = jev_client.clone();
                        let state_clone = state.clone();

                        tokio::spawn(async move {
                            let _permit = permit;
                            if let Err(e) = handle_connection(stream, rx_clone, tx_clone, jev_clone, state_clone).await {
                                debug!("Connection handling error for {}: {}", peer_addr, e);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("Accept error in web dashboard: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_connection(
    mut stream: TcpStream,
    status_rx: watch::Receiver<Arc<PublishedStatus>>,
    status_tx: watch::Sender<Arc<PublishedStatus>>,
    jev_client: Arc<JevClient>,
    state: Arc<RwLock<DynamicWebState>>,
) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];

    // Read headers bounded by time and size
    let read_res = timeout(READ_TIMEOUT, async {
        loop {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);

            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                if pos + 4 > MAX_HEADER_SIZE {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Header size exceeded limit",
                    ));
                }
                break;
            } else if buf.len() > MAX_HEADER_SIZE {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Header size exceeded limit",
                ));
            }
        }
        Ok::<_, std::io::Error>(())
    })
    .await;

    if read_res.is_err() {
        let resp =
            b"HTTP/1.1 408 Request Timeout\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
        return Ok(());
    }

    if let Ok(Err(_)) = read_res {
        let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 11\r\nConnection: close\r\n\r\nBad Request";
        let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
        return Ok(());
    }

    let header_end_pos = match buf.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(pos) => pos,
        None => {
            let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 26\r\nConnection: close\r\n\r\nIncomplete Request Headers";
            let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
            return Ok(());
        }
    };
    let header_str = String::from_utf8_lossy(&buf[..header_end_pos]).to_string();

    let mut header_lines = header_str.lines();
    let request_line = match header_lines.next() {
        Some(l) => l.trim(),
        None => {
            let resp =
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
            return Ok(());
        }
    };

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
        return Ok(());
    }

    let method = parts[0];
    let path = parts[1];
    let clean_path = path.split('?').next().unwrap_or(path);
    let is_head = method == "HEAD";

    // Validate header field lines and extract Content-Length & Transfer-Encoding per RFC 9112 §5.1
    let mut cl_value: Option<usize> = None;
    let mut has_unsupported_te = false;

    for line in header_lines {
        let trimmed_line = line.trim_end_matches('\r');
        if trimmed_line.is_empty() {
            continue;
        }

        // RFC 9112 Section 5.1: No whitespace is allowed between the field-name and colon
        let Some((field_name, field_val)) = trimmed_line.split_once(':') else {
            let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 26\r\nConnection: close\r\n\r\nInvalid Header Field Line";
            let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
            return Ok(());
        };

        if field_name.bytes().any(|b| b == b' ' || b == b'\t') {
            let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 28\r\nConnection: close\r\n\r\nInvalid Header Field Syntax";
            let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
            return Ok(());
        }

        let name_lower = field_name.to_lowercase();
        let val_trimmed = field_val.trim_matches([' ', '\t']);

        if name_lower == "transfer-encoding" {
            has_unsupported_te = true;
        } else if name_lower == "content-length" {
            if cl_value.is_some() {
                let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 24\r\nConnection: close\r\n\r\nDuplicate Content-Length";
                let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
                return Ok(());
            }
            if val_trimmed.is_empty() || !val_trimmed.bytes().all(|b| b.is_ascii_digit()) {
                let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 22\r\nConnection: close\r\n\r\nInvalid Content-Length";
                let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
                return Ok(());
            }
            match val_trimmed.parse::<usize>() {
                Ok(len) => cl_value = Some(len),
                Err(_) => {
                    let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 22\r\nConnection: close\r\n\r\nInvalid Content-Length";
                    let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
                    return Ok(());
                }
            }
        }
    }

    // Read POST/DELETE body if applicable
    let mut body_bytes = Vec::new();
    if method == "POST" || method == "DELETE" {
        if has_unsupported_te {
            let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 29\r\nConnection: close\r\n\r\nUnsupported Transfer-Encoding";
            let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
            return Ok(());
        }

        let content_length = cl_value.unwrap_or(0);

        if content_length > MAX_BODY_SIZE {
            let resp = b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 17\r\nConnection: close\r\n\r\nPayload Too Large";
            let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
            return Ok(());
        }

        // Bound leftover bytes strictly to content_length
        let leftover = &buf[header_end_pos + 4..];
        let take_len = leftover.len().min(content_length);
        body_bytes.extend_from_slice(&leftover[..take_len]);

        // Read remaining body within dedicated BODY_READ_TIMEOUT
        if body_bytes.len() < content_length {
            let read_res = timeout(BODY_READ_TIMEOUT, async {
                while body_bytes.len() < content_length {
                    let remaining = content_length - body_bytes.len();
                    let mut read_chunk = vec![0u8; remaining.min(1024)];
                    let n = stream.read(&mut read_chunk).await?;
                    if n == 0 {
                        break; // Premature EOF
                    }
                    body_bytes.extend_from_slice(&read_chunk[..n]);
                }
                Ok::<(), std::io::Error>(())
            })
            .await;

            if read_res.is_err() {
                let resp = b"HTTP/1.1 408 Request Timeout\r\nContent-Length: 15\r\nConnection: close\r\n\r\nRequest Timeout";
                let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
                return Ok(());
            }
        }

        // Verify body completeness before router
        if body_bytes.len() != content_length {
            let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 15\r\nConnection: close\r\n\r\nIncomplete Body";
            let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
            return Ok(());
        }
    }

    // Router
    match (method, clean_path) {
        ("GET", "/") | ("GET", "/index.html") | ("HEAD", "/") | ("HEAD", "/index.html") => {
            let body = DASHBOARD_HTML.as_bytes();
            let header = format!(
                "HTTP/1.1 200 OK\r\n\
                Content-Type: text/html; charset=utf-8\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                Cache-Control: no-cache, no-store, must-revalidate\r\n\
                Pragma: no-cache\r\n\
                Expires: 0\r\n\
                X-Content-Type-Options: nosniff\r\n\
                \r\n",
                body.len()
            );

            let _ = timeout(WRITE_TIMEOUT, async {
                stream.write_all(header.as_bytes()).await?;
                if !is_head {
                    stream.write_all(body).await?;
                }
                stream.flush().await
            })
            .await;
        }

        ("GET", "/dashboard.css")
        | ("HEAD", "/dashboard.css")
        | ("GET", "/dashboard.js")
        | ("HEAD", "/dashboard.js") => {
            let (body, content_type) = if clean_path.ends_with(".css") {
                (DASHBOARD_CSS, "text/css; charset=utf-8")
            } else {
                (DASHBOARD_JS, "text/javascript; charset=utf-8")
            };
            let header = format!("HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\n\r\n", content_type, body.len());
            let _ = timeout(WRITE_TIMEOUT, async {
                stream.write_all(header.as_bytes()).await?;
                if !is_head {
                    stream.write_all(body.as_bytes()).await?;
                }
                stream.flush().await
            })
            .await;
        }

        ("GET", "/i18n.js") | ("HEAD", "/i18n.js") => {
            let body = I18N_JS.as_bytes();
            let header = format!(
                "HTTP/1.1 200 OK\r\n\
                Content-Type: text/javascript; charset=utf-8\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                Cache-Control: no-cache\r\n\
                X-Content-Type-Options: nosniff\r\n\
                \r\n",
                body.len()
            );
            let _ = timeout(WRITE_TIMEOUT, async {
                stream.write_all(header.as_bytes()).await?;
                if !is_head {
                    stream.write_all(body).await?;
                }
                stream.flush().await
            })
            .await;
        }

        ("GET", path) | ("HEAD", path)
            if matches!(
                path,
                "/locales/en.json"
                    | "/locales/ru.json"
                    | "/locales/de.json"
                    | "/locales/fr.json"
                    | "/locales/es.json"
                    | "/locales/pt-BR.json"
                    | "/locales/zh-CN.json"
                    | "/locales/ja.json"
            ) =>
        {
            let body_str: &'static str = match path {
                "/locales/en.json" => LOCALE_EN,
                "/locales/ru.json" => LOCALE_RU,
                "/locales/de.json" => LOCALE_DE,
                "/locales/fr.json" => LOCALE_FR,
                "/locales/es.json" => LOCALE_ES,
                "/locales/pt-BR.json" => LOCALE_PT_BR,
                "/locales/zh-CN.json" => LOCALE_ZH_CN,
                "/locales/ja.json" => LOCALE_JA,
                _ => unreachable!(),
            };
            let body = body_str.as_bytes();
            let header = format!(
                "HTTP/1.1 200 OK\r\n\
                Content-Type: application/json; charset=utf-8\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                Cache-Control: no-cache\r\n\
                X-Content-Type-Options: nosniff\r\n\
                \r\n",
                body.len()
            );
            let _ = timeout(WRITE_TIMEOUT, async {
                stream.write_all(header.as_bytes()).await?;
                if !is_head {
                    stream.write_all(body).await?;
                }
                stream.flush().await
            })
            .await;
        }

        ("GET", "/api/status") | ("HEAD", "/api/status") => {
            let current_status = {
                let borrowed = status_rx.borrow();
                Arc::clone(&*borrowed)
            };

            let json_body =
                serde_json::to_string(&current_status.refreshed_at(std::time::Instant::now()))
                    .unwrap_or_else(|_| "{}".to_string());
            let body = json_body.as_bytes();
            let header = format!(
                "HTTP/1.1 200 OK\r\n\
                Content-Type: application/json; charset=utf-8\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                Cache-Control: no-cache, no-store, must-revalidate\r\n\
                Pragma: no-cache\r\n\
                Expires: 0\r\n\
                X-Content-Type-Options: nosniff\r\n\
                Access-Control-Allow-Origin: *\r\n\
                \r\n",
                body.len()
            );

            let _ = timeout(WRITE_TIMEOUT, async {
                stream.write_all(header.as_bytes()).await?;
                if !is_head {
                    stream.write_all(body).await?;
                }
                stream.flush().await
            })
            .await;
        }

        ("GET", "/api/categories") => {
            let cats = {
                let st = state.read().await;
                st.categories.clone()
            };
            let json_body = serde_json::to_string(&cats).unwrap_or_else(|_| "[]".to_string());
            send_json_response(&mut stream, 200, &json_body).await;
        }

        ("POST", "/api/categories") | ("DELETE", "/api/categories") => {
            #[derive(serde::Deserialize)]
            struct CategoryReq {
                id: String,
                #[serde(default)]
                label: Option<String>,
                #[serde(default)]
                description: Option<String>,
                #[serde(default)]
                delete: Option<bool>,
            }

            let is_delete_request = method == "DELETE";

            let parsed_req = if is_delete_request && body_bytes.is_empty() {
                let q_id = path.split('?').nth(1).and_then(|q| {
                    q.split('&').find_map(|pair| {
                        let mut it = pair.split('=');
                        if it.next() == Some("id") {
                            it.next().map(|v| v.to_string())
                        } else {
                            None
                        }
                    })
                });
                match q_id {
                    Some(id) => Ok((id, true, None, None)),
                    None => Err(("Missing category id in DELETE request", "missing_id")),
                }
            } else {
                match serde_json::from_slice::<CategoryReq>(&body_bytes) {
                    Ok(req) => {
                        let del = is_delete_request || req.delete == Some(true);
                        Ok((req.id, del, req.label, req.description))
                    }
                    Err(_e) => Err(("Invalid JSON", "invalid_json")),
                }
            };

            let (target_id, is_delete, label_opt, desc_opt) = match parsed_req {
                Ok(tuple) => tuple,
                Err((msg, code)) => {
                    let err_json = serde_json::json!({
                        "error": msg,
                        "error_code": code
                    });
                    send_json_response(&mut stream, 400, &err_json.to_string()).await;
                    return Ok(());
                }
            };

            let clean_id = target_id.trim();
            if clean_id.is_empty() {
                let err_json = serde_json::json!({
                    "error": "Category ID cannot be empty",
                    "error_code": "invalid_id"
                });
                send_json_response(&mut stream, 400, &err_json.to_string()).await;
                return Ok(());
            }

            if is_delete {
                let updated_cats = {
                    let mut st = state.write().await;
                    st.categories.retain(|c| c.id != clean_id);
                    st.category_map.retain(|_, v| v != clean_id);
                    st.categories.clone()
                };

                // Synchronize published status so dashboard updates immediately
                status_tx.send_modify(|current| {
                    let updated = Arc::make_mut(current);
                    updated.categories = updated_cats.clone();
                    for t in &mut updated.targets {
                        if t.category.as_deref() == Some(clean_id) {
                            t.category = None;
                        }
                    }
                    updated.view_revision = updated
                        .view_revision
                        .parse::<u64>()
                        .unwrap_or(0)
                        .saturating_add(1)
                        .to_string();
                });

                let json_body = serde_json::to_string(&updated_cats).unwrap_or_default();
                send_json_response(&mut stream, 200, &json_body).await;
            } else {
                let label = match label_opt {
                    Some(l) if !l.trim().is_empty() => l.trim().to_string(),
                    _ => {
                        let err_json = serde_json::json!({
                            "error": "Category label is required",
                            "error_code": "missing_label"
                        });
                        send_json_response(&mut stream, 400, &err_json.to_string()).await;
                        return Ok(());
                    }
                };
                let description = desc_opt.unwrap_or_default().trim().to_string();

                let new_cat = CategoryDefinition {
                    id: clean_id.to_string(),
                    label,
                    description,
                    label_key: None,
                };

                let updated_cats = {
                    let mut st = state.write().await;
                    if let Some(pos) = st.categories.iter().position(|c| c.id == new_cat.id) {
                        st.categories[pos] = new_cat;
                    } else {
                        st.categories.push(new_cat);
                    }
                    st.categories.clone()
                };

                // Synchronize published status so dashboard receives new category tab immediately
                status_tx.send_modify(|current| {
                    let updated = Arc::make_mut(current);
                    updated.categories = updated_cats.clone();
                    updated.view_revision = updated
                        .view_revision
                        .parse::<u64>()
                        .unwrap_or(0)
                        .saturating_add(1)
                        .to_string();
                });

                let json_body = serde_json::to_string(&updated_cats).unwrap_or_default();
                send_json_response(&mut stream, 200, &json_body).await;
            }
        }

        ("POST", "/api/categorize") => {
            let (targets, categories, active_key) = {
                let current_status = {
                    let borrowed = status_rx.borrow();
                    Arc::clone(&*borrowed)
                };
                let st = state.read().await;
                (
                    current_status.targets.clone(),
                    st.categories.clone(),
                    st.custom_api_key.clone(),
                )
            };

            match jev_client
                .categorize_targets(&targets, &categories, active_key.as_deref())
                .await
            {
                Ok(mapping) => {
                    // Update state map
                    {
                        let mut st = state.write().await;
                        for (k, v) in &mapping {
                            st.category_map.insert(k.clone(), v.clone());
                        }
                    }

                    // Update published status so dashboard updates on next fetch
                    status_tx.send_modify(|current| {
                        let updated = Arc::make_mut(current);
                        for t in &mut updated.targets {
                            if let Some(cat) = mapping.get(&t.name) {
                                t.category = Some(cat.clone());
                            }
                        }
                        updated.view_revision = updated
                            .view_revision
                            .parse::<u64>()
                            .unwrap_or(0)
                            .saturating_add(1)
                            .to_string();
                    });

                    let resp = serde_json::json!({
                        "success": true,
                        "categorized_count": mapping.len(),
                        "mapping": mapping
                    });
                    send_json_response(&mut stream, 200, &resp.to_string()).await;
                }
                Err(e) => {
                    let err_resp = serde_json::json!({
                        "success": false,
                        "error": format!("TypeSafe Jev API error: {}", e),
                        "error_code": "jev_api"
                    });
                    send_json_response(&mut stream, 502, &err_resp.to_string()).await;
                }
            }
        }

        ("GET", "/api/key-status") => {
            let default_key = jev_client.api_key();
            let st = state.read().await;
            let has_custom = st.custom_api_key.is_some();
            let active_key = st.custom_api_key.as_deref().unwrap_or(default_key);

            let resp = KeyStatusResponse {
                has_custom_key: has_custom,
                active_key_masked: mask_key(active_key),
                default_key_masked: mask_key(default_key),
            };
            let json_body = serde_json::to_string(&resp).unwrap_or_default();
            send_json_response(&mut stream, 200, &json_body).await;
        }

        ("POST", "/api/key") => {
            let parsed: std::result::Result<SetKeyRequest, _> = serde_json::from_slice(&body_bytes);
            match parsed {
                Ok(req) => {
                    if let Some(ref new_key) = req.api_key {
                        let trimmed = new_key.trim();
                        if trimmed.is_empty() {
                            // Reset to default
                            let mut st = state.write().await;
                            st.custom_api_key = None;
                        } else {
                            // Verify key before applying
                            match jev_client.verify_api_key(trimmed).await {
                                Ok(valid) => {
                                    if valid {
                                        let mut st = state.write().await;
                                        st.custom_api_key = Some(trimmed.to_string());
                                    } else {
                                        let err_resp = serde_json::json!({
                                            "error": "API key verification failed (rejected by Jev API)",
                                            "error_code": "key_rejected"
                                        });
                                        send_json_response(&mut stream, 400, &err_resp.to_string())
                                            .await;
                                        return Ok(());
                                    }
                                }
                                Err(e) => {
                                    let err_resp = serde_json::json!({
                                        "error": format!("Jev verification error: {}", e),
                                        "error_code": "key_verification"
                                    });
                                    send_json_response(&mut stream, 502, &err_resp.to_string())
                                        .await;
                                    return Ok(());
                                }
                            }
                        }
                    } else {
                        // None means reset to default
                        let mut st = state.write().await;
                        st.custom_api_key = None;
                    }

                    let default_key = jev_client.api_key();
                    let st = state.read().await;
                    let has_custom = st.custom_api_key.is_some();
                    let active_key = st.custom_api_key.as_deref().unwrap_or(default_key);

                    let resp = KeyStatusResponse {
                        has_custom_key: has_custom,
                        active_key_masked: mask_key(active_key),
                        default_key_masked: mask_key(default_key),
                    };
                    let json_body = serde_json::to_string(&resp).unwrap_or_default();
                    send_json_response(&mut stream, 200, &json_body).await;
                }
                Err(e) => {
                    let err_json = serde_json::json!({
                        "error": format!("Invalid JSON: {}", e),
                        "error_code": "invalid_json"
                    });
                    send_json_response(&mut stream, 400, &err_json.to_string()).await;
                }
            }
        }

        _ => {
            let resp = b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found";
            let _ = timeout(WRITE_TIMEOUT, stream.write_all(resp)).await;
        }
    }

    Ok(())
}

async fn send_json_response(stream: &mut TcpStream, status_code: u16, json_body: &str) {
    let body = json_body.as_bytes();
    let status_text = match status_code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    };

    let header = format!(
        "HTTP/1.1 {} {}\r\n\
        Content-Type: application/json; charset=utf-8\r\n\
        Content-Length: {}\r\n\
        Connection: close\r\n\
        Cache-Control: no-store\r\n\
        X-Content-Type-Options: nosniff\r\n\
        Access-Control-Allow-Origin: *\r\n\
        \r\n",
        status_code,
        status_text,
        body.len()
    );

    let _ = timeout(WRITE_TIMEOUT, async {
        stream.write_all(header.as_bytes()).await?;
        stream.write_all(body).await?;
        stream.flush().await
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn test_web_server_endpoints() {
        let (status_tx, status_rx) = watch::channel(Arc::new(PublishedStatus {
            cycle_id: 42,
            timestamp: chrono::Utc::now(),
            phase: "ready".to_string(),
            system_health: "healthy".to_string(),
            health_confidence: Some(0.99),
            risk_score: Some(0.05),
            action_required: Some(false),
            suggested_action: Some("none".to_string()),
            jev_latency_ms: Some(120.0),
            collection_latency_ms: Some(350.0),
            online_targets: 1,
            total_targets: 1,
            categories: default_categories(),
            targets: vec![],
            error_message: None,
            ..PublishedStatus::default()
        }));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let jev_client = Arc::new(
            JevClient::new("test-key".to_string(), None, None, None, None)
                .expect("Failed to create client"),
        );

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind ephemeral listener");
        let port = listener.local_addr().unwrap().port();
        let addr = format!("127.0.0.1:{}", port);

        let server_handle = tokio::spawn(async move {
            run_web_server_with_listener(listener, status_rx, status_tx, jev_client, shutdown_rx)
                .await
        });

        // 1. Test GET / (HTML)
        let mut client = TcpStream::connect(&addr).await.expect("Failed to connect");
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("Content-Type: text/html"));
        assert!(resp.contains("Jev Sentinel"));

        for (route, content_type) in [
            ("/dashboard.css", "text/css"),
            ("/dashboard.js", "text/javascript"),
        ] {
            let mut client = TcpStream::connect(&addr).await.unwrap();
            client
                .write_all(format!("GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n", route).as_bytes())
                .await
                .unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).await.unwrap();
            assert!(resp.starts_with("HTTP/1.1 200 OK"));
            assert!(resp.contains(content_type));
        }
        let mut client = TcpStream::connect(&addr).await.unwrap();
        client
            .write_all(b"GET /api/status HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        let body = resp.split_once("\r\n\r\n").unwrap().1;
        let payload: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(payload["schema_version"], 2);
        assert!(payload["view_revision"].is_string());
        assert_eq!(payload["risk_score"], 0.05);

        // 2. Test GET /api/categories (JSON)
        let mut client = TcpStream::connect(&addr).await.expect("Failed to connect");
        client
            .write_all(b"GET /api/categories HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("ai_compute"));

        // 3. Test GET /api/key-status
        let mut client = TcpStream::connect(&addr).await.expect("Failed to connect");
        client
            .write_all(b"GET /api/key-status HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("default_key_masked"));

        // 4. Test POST /api/categories (Add Category)
        let mut client = TcpStream::connect(&addr).await.expect("Failed to connect");
        let payload =
            r#"{"id":"databases","label":"Database Clusters","description":"Postgres and Redis"}"#;
        let req = format!(
            "POST /api/categories HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
            payload.len(),
            payload
        );
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("databases"));

        // 4a. Test POST /api/categories with delete: true
        let mut client = TcpStream::connect(&addr).await.expect("Failed to connect");
        let del_payload = r#"{"id":"databases","delete":true}"#;
        let req = format!(
            "POST /api/categories HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
            del_payload.len(),
            del_payload
        );
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(!resp.contains("databases"));

        // 4b. Re-add and Test DELETE /api/categories?id=databases
        let mut client = TcpStream::connect(&addr).await.expect("Failed to connect");
        let req_add = format!(
            "POST /api/categories HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
            payload.len(),
            payload
        );
        client.write_all(req_add.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.contains("databases"));

        let mut client = TcpStream::connect(&addr).await.expect("Failed to connect");
        let req_del = "DELETE /api/categories?id=databases HTTP/1.1\r\nHost: localhost\r\n\r\n";
        client.write_all(req_del.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(!resp.contains("databases"));

        // 5. Test POST /api/key (Reset to default)
        let mut client = TcpStream::connect(&addr).await.expect("Failed to connect");
        let key_payload = r#"{"api_key":null}"#;
        let req = format!(
            "POST /api/key HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
            key_payload.len(),
            key_payload
        );
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("\"has_custom_key\":false"));

        // Shutdown
        shutdown_tx.send(true).unwrap();
        let _ = server_handle.await;
    }

    #[test]
    fn test_mask_key_logic() {
        assert_eq!(mask_key("apikey_2238ba52dd6634bf465d"), "apikey_2...465d");
        assert_eq!(mask_key("short"), "***");
        assert_eq!(mask_key(""), "none");
        assert_eq!(mask_key("🔒секретный-токен"), "***");
    }

    #[tokio::test]
    async fn test_i18n_and_locales_routes_and_security() {
        let (status_tx, status_rx) = watch::channel(Arc::new(PublishedStatus::default()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let jev_client = Arc::new(
            JevClient::new("test-key".to_string(), None, None, None, None)
                .expect("Failed to create client"),
        );

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind ephemeral listener");
        let port = listener.local_addr().unwrap().port();
        let addr = format!("127.0.0.1:{}", port);

        let server_handle = tokio::spawn(async move {
            run_web_server_with_listener(listener, status_rx, status_tx, jev_client, shutdown_rx)
                .await
        });

        // 1. Test GET /i18n.js
        let mut client = TcpStream::connect(&addr).await.unwrap();
        client
            .write_all(b"GET /i18n.js HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("Content-Type: text/javascript; charset=utf-8"));
        assert!(resp.contains("X-Content-Type-Options: nosniff"));
        assert!(resp.contains("Cache-Control: no-cache"));
        let (_header, body) = resp.split_once("\r\n\r\n").unwrap();
        assert!(body.starts_with("globalThis.SentinelEnglish = "));
        assert!(body.contains("app.title"));

        // 2. Test HEAD /i18n.js
        let mut client = TcpStream::connect(&addr).await.unwrap();
        client
            .write_all(b"HEAD /i18n.js HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("Content-Type: text/javascript; charset=utf-8"));
        assert!(resp.contains("X-Content-Type-Options: nosniff"));
        let (_, head_body) = resp.split_once("\r\n\r\n").unwrap();
        assert_eq!(head_body, "");

        // 3. Test 8 fixed whitelist locales: GET and HEAD
        let locales = ["en", "ru", "de", "fr", "es", "pt-BR", "zh-CN", "ja"];
        for loc in locales {
            let path = format!("/locales/{}.json", loc);

            // GET
            let mut client = TcpStream::connect(&addr).await.unwrap();
            client
                .write_all(format!("GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n", path).as_bytes())
                .await
                .unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).await.unwrap();
            assert!(resp.starts_with("HTTP/1.1 200 OK"), "Route {} failed", path);
            assert!(resp.contains("Content-Type: application/json; charset=utf-8"));
            assert!(resp.contains("X-Content-Type-Options: nosniff"));
            assert!(resp.contains("Cache-Control: no-cache"));
            let (_, body) = resp.split_once("\r\n\r\n").unwrap();
            let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
            assert!(parsed.get("app.title").is_some());

            // HEAD
            let mut client = TcpStream::connect(&addr).await.unwrap();
            client
                .write_all(format!("HEAD {} HTTP/1.1\r\nHost: localhost\r\n\r\n", path).as_bytes())
                .await
                .unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).await.unwrap();
            assert!(resp.starts_with("HTTP/1.1 200 OK"));
            assert!(resp.contains("Content-Type: application/json; charset=utf-8"));
            let (_, head_body) = resp.split_once("\r\n\r\n").unwrap();
            assert_eq!(head_body, "");
        }

        // 4. Test unknown locales and traversal attempts return 404
        for bad_path in [
            "/locales/it.json",
            "/locales/fr",
            "/locales/zh-TW.json",
            "/locales/en.json/more",
            "/locales/",
            "/locales/../locales/en.json",
        ] {
            let mut client = TcpStream::connect(&addr).await.unwrap();
            client
                .write_all(
                    format!("GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n", bad_path).as_bytes(),
                )
                .await
                .unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).await.unwrap();
            assert!(
                resp.starts_with("HTTP/1.1 404 Not Found"),
                "Path {} should be 404",
                bad_path
            );

            let mut client = TcpStream::connect(&addr).await.unwrap();
            client
                .write_all(
                    format!("HEAD {} HTTP/1.1\r\nHost: localhost\r\n\r\n", bad_path).as_bytes(),
                )
                .await
                .unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).await.unwrap();
            assert!(resp.starts_with("HTTP/1.1 404 Not Found"));
        }

        // 5. Preserving user category labels and stripping untrusted label_key
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let cat_payload = r#"{"id":"custom_ops","label":"Custom DevOps","description":"CI/CD","label_key":"category.observability"}"#;
        let req = format!(
            "POST /api/categories HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
            cat_payload.len(),
            cat_payload
        );
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        let body = resp.split_once("\r\n\r\n").unwrap().1;
        let cats: Vec<serde_json::Value> = serde_json::from_str(body).unwrap();
        let custom_cat = cats
            .iter()
            .find(|c| c["id"] == "custom_ops")
            .expect("custom_ops found");
        // User label is strictly preserved
        assert_eq!(custom_cat["label"], "Custom DevOps");
        // Untrusted label_key is stripped
        assert!(custom_cat.get("label_key").is_none() || custom_cat["label_key"].is_null());

        // Builtin category retains label_key
        let builtin_cat = cats
            .iter()
            .find(|c| c["id"] == "observability")
            .expect("observability found");
        assert_eq!(builtin_cat["label_key"], "category.observability");

        // 6. Test JSON POST error responses with stable error_code
        // Invalid JSON in POST /api/categories
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let req = "POST /api/categories HTTP/1.1\r\nHost: localhost\r\nContent-Length: 12\r\nContent-Type: application/json\r\n\r\nnot-valid-json";
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 400 Bad Request"));
        let body = resp.split_once("\r\n\r\n").unwrap().1;
        let err_obj: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(err_obj["error_code"], "invalid_json");
        assert!(err_obj["error"].as_str().unwrap().contains("Invalid JSON"));

        // Invalid JSON in POST /api/key
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let req = "POST /api/key HTTP/1.1\r\nHost: localhost\r\nContent-Length: 12\r\nContent-Type: application/json\r\n\r\nnot-valid-json";
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 400 Bad Request"));
        let body = resp.split_once("\r\n\r\n").unwrap().1;
        let err_obj: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(err_obj["error_code"], "invalid_json");

        // 7. Test incomplete body (premature EOF) rejects without mutating state
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let req = "POST /api/categories HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\nContent-Type: application/json\r\n\r\n{\"id\":\"incomplete\"";
        client.write_all(req.as_bytes()).await.unwrap();
        client.shutdown().await.unwrap(); // simulate premature EOF
        let mut resp = String::new();
        let _ = client.read_to_string(&mut resp).await;
        assert!(resp.contains("400 Bad Request"));
        assert!(resp.contains("Incomplete Body"));

        // 7a. Test incomplete headers (EOF without \r\n\r\n) rejects without mutating state
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let req = "DELETE /api/categories?id=ai_compute HTTP/1.1\r\nHost: localhost\r\n";
        client.write_all(req.as_bytes()).await.unwrap();
        client.shutdown().await.unwrap();
        let mut resp = String::new();
        let _ = client.read_to_string(&mut resp).await;
        assert!(resp.contains("400 Bad Request"));
        assert!(resp.contains("Incomplete Request Headers"));

        // 8. Test duplicate Content-Length rejected
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let req = "POST /api/categories HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10\r\nContent-Length: 10\r\nContent-Type: application/json\r\n\r\n{}";
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.contains("400 Bad Request"));
        assert!(resp.contains("Duplicate Content-Length"));

        // 8a. Test malformed Content-Length with colon/garbage rejected without mutating state
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let req = "DELETE /api/categories?id=ai_compute HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0:garbage\r\n\r\n";
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.contains("400 Bad Request"));
        assert!(resp.contains("Invalid Content-Length"));

        // Verify ai_compute was not deleted by malformed/incomplete requests
        let mut client = TcpStream::connect(&addr).await.unwrap();
        client
            .write_all(b"GET /api/categories HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.contains("ai_compute"));

        // 9. Test unsupported Transfer-Encoding rejected
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let req = "POST /api/categories HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n0\r\n\r\n";
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.contains("400 Bad Request"));
        assert!(resp.contains("Unsupported Transfer-Encoding"));

        // 10. Test fragmented TCP delivery (1 byte chunks) succeeds
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let fragment_payload =
            r#"{"id":"fragmented","label":"Fragmented","description":"Byte by byte"}"#;
        let req = format!(
            "POST /api/categories HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
            fragment_payload.len(),
            fragment_payload
        );
        for byte in req.as_bytes() {
            client.write_all(&[*byte]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("fragmented"));

        // 8b. Test whitespace before colon in Content-Length (RFC 9112 §5.1) rejected
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let req = "DELETE /api/categories?id=ai_compute HTTP/1.1\r\nHost: localhost\r\nContent-Length : 0:garbage\r\n\r\n";
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.contains("400 Bad Request"));
        assert!(resp.contains("Invalid Header Field Syntax"));

        // 8c. Test plus sign in Content-Length rejected
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let req = "DELETE /api/categories?id=ai_compute HTTP/1.1\r\nHost: localhost\r\nContent-Length: +0\r\n\r\n";
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.contains("400 Bad Request"));
        assert!(resp.contains("Invalid Content-Length"));

        // 9a. Test whitespace before colon in Transfer-Encoding rejected
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let req = "DELETE /api/categories?id=ai_compute HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding : chunked\r\n\r\n";
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.contains("400 Bad Request"));
        assert!(resp.contains("Invalid Header Field Syntax"));

        // 11. Test 8190-byte headers with body coalesced in single read succeeds
        let mut client = TcpStream::connect(&addr).await.unwrap();
        let body = r#"{"id":"coalesced","label":"Coalesced","description":"Valid"}"#;
        let prefix = format!(
            "POST /api/categories HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\nX-Pad: ",
            body.len()
        );
        let pad_len = 8190usize.saturating_sub(prefix.len() + 4);
        let header_str = format!("{}{}\r\n\r\n", prefix, "a".repeat(pad_len));
        assert_eq!(header_str.len(), 8190);
        let mut full_req = header_str.into_bytes();
        full_req.extend_from_slice(body.as_bytes());
        client.write_all(&full_req).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("coalesced"));

        // Verify ai_compute was never deleted by any of the 8b, 8c, 9a malformed requests
        let mut client = TcpStream::connect(&addr).await.unwrap();
        client
            .write_all(b"GET /api/categories HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        assert!(resp.contains("ai_compute"));

        shutdown_tx.send(true).unwrap();
        let _ = server_handle.await;
    }
}
