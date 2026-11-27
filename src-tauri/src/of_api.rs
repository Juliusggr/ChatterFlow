use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tauri::AppHandle;
use tokio::sync::{oneshot, Semaphore};

use crate::sessions;

static REQ_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct BridgeReply {
    status: u16,
    body: String,
    error: String,
}

fn pending_replies() -> &'static Mutex<HashMap<String, oneshot::Sender<BridgeReply>>> {
    static MAP: OnceLock<Mutex<HashMap<String, oneshot::Sender<BridgeReply>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn model_limiter(model_id: &str) -> Arc<Semaphore> {
    static LIMITERS: OnceLock<Mutex<HashMap<String, Arc<Semaphore>>>> = OnceLock::new();
    let map = LIMITERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|poison| poison.into_inner());
    guard
        .entry(model_id.to_owned())
        .or_insert_with(|| Arc::new(Semaphore::new(2)))
        .clone()
}

#[tauri::command]
pub fn of_api_bridge_reply(req_id: String, status: u16, body: String, error: String) {
    if let Some(sender) = pending_replies()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .remove(&req_id)
    {
        let _ = sender.send(BridgeReply { status, body, error });
    }
}

pub async fn request_raw(
    app: &AppHandle,
    model_id: &str,
    method: &str,
    path: &str,
    payload: Option<Value>,
) -> Result<String, String> {
    if !path.starts_with("/api2/v2/") {
        return Err("Ruta de API no permitida".to_owned());
    }
    let mut attempt = 0_u32;
    loop {
        let limiter = model_limiter(model_id);
        let permit = limiter.acquire().await.map_err(|err| err.to_string())?;
        let result = request_once(app, model_id, method, path, payload.clone()).await;
        drop(permit);
        match result {
            Ok(body) => return Ok(body),
            Err(err) if is_rate_limit(&err) && attempt < 2 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(800 * u64::from(attempt))).await;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn request_once(
    app: &AppHandle,
    model_id: &str,
    method: &str,
    path: &str,
    payload: Option<Value>,
) -> Result<String, String> {
    let webview = sessions::onlyfans_webview(app, model_id)?;
    let req_id = format!("req-{}", REQ_SEQ.fetch_add(1, Ordering::Relaxed));
    let (sender, receiver) = oneshot::channel();
    pending_replies()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(req_id.clone(), sender);

    let js = format!(
        "window.__CF_API_CALL && window.__CF_API_CALL({}, {}, {}, {});",
        serde_json::to_string(&req_id).unwrap_or_else(|_| "\"\"".to_owned()),
        serde_json::to_string(method).unwrap_or_else(|_| "\"GET\"".to_owned()),
        serde_json::to_string(path).unwrap_or_else(|_| "\"\"".to_owned()),
        serde_json::to_string(&payload.unwrap_or(Value::Null)).unwrap_or_else(|_| "null".to_owned()),
    );
    if let Err(err) = webview.eval(&js) {
        pending_replies()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&req_id);
        return Err(err.to_string());
    }

    let reply = match tokio::time::timeout(Duration::from_secs(20), receiver).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(_)) => return Err("Se cerró la ventana del modelo durante la petición".to_owned()),
        Err(_) => {
            pending_replies()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(&req_id);
            return Err("Timeout esperando respuesta de OnlyFans".to_owned());
        }
    };
    if !reply.error.is_empty() {
        return Err(reply.error);
    }
    match reply.status {
        200..=299 => Ok(reply.body),
        401 | 403 => Err("Sesión expirada o no autenticada en OnlyFans".to_owned()),
        429 => Err("Rate limit temporal de OnlyFans (429)".to_owned()),
        code => Err(format!("OnlyFans respondió HTTP {code}")),
    }
}

fn is_rate_limit(err: &str) -> bool {
    err.contains("429") || err.to_ascii_lowercase().contains("rate limit")
}

pub async fn get_json<T: DeserializeOwned>(
    app: &AppHandle,
    model_id: &str,
    path: &str,
) -> Result<T, String> {
    let raw = request_raw(app, model_id, "GET", path, None).await?;
    serde_json::from_str(&raw).map_err(|err| format!("JSON inválido de OnlyFans: {err}"))
}

pub async fn get_me(app: &AppHandle, model_id: &str) -> Result<Value, String> {
    get_json(app, model_id, "/api2/v2/users/me").await
}

pub async fn get_balances(app: &AppHandle, model_id: &str) -> Result<Value, String> {
    get_json(app, model_id, "/api2/v2/payouts/balances").await
}

pub async fn list_transactions(
    app: &AppHandle,
    model_id: &str,
    limit: u32,
    marker: Option<&str>,
) -> Result<Value, String> {
    let path = match marker {
        Some(marker) if !marker.is_empty() => format!(
            "/api2/v2/payouts/transactions?limit={}&marker={}",
            limit.min(50),
            urlencoding::encode(marker)
        ),
        _ => format!("/api2/v2/payouts/transactions?limit={}", limit.min(50)),
    };
    get_json(app, model_id, &path).await
}

#[allow(dead_code)]
pub async fn list_chats(app: &AppHandle, model_id: &str, limit: u32, offset: u32) -> Result<Value, String> {
    let path = format!(
        "/api2/v2/chats?limit={}&offset={}&order=recent",
        limit.min(10),
        offset
    );
    get_json(app, model_id, &path).await
}

#[allow(dead_code)]
pub async fn list_messages(
    app: &AppHandle,
    model_id: &str,
    fan_id: &str,
    id_before: Option<&str>,
) -> Result<Value, String> {
    let clean = fan_id.trim().trim_start_matches(['u', 'U']);
    if clean.is_empty() || !clean.chars().all(|ch| ch.is_ascii_digit()) {
        return Err("ID de fan inválido".to_owned());
    }
    let path = match id_before.filter(|value| !value.is_empty()) {
        Some(before) => format!(
            "/api2/v2/chats/{clean}/messages?limit=10&order=desc&id={before}"
        ),
        None => format!("/api2/v2/chats/{clean}/messages?limit=10&order=desc"),
    };
    get_json(app, model_id, &path).await
}

pub async fn list_subscribers(
    app: &AppHandle,
    model_id: &str,
    filter: &str,
    limit: u32,
    offset: u32,
) -> Result<Value, String> {
    let kind = match filter {
        "active" | "expired" => filter,
        _ => "all",
    };
    let path = format!(
        "/api2/v2/subscriptions/subscribers?limit={}&offset={}&type={kind}&sort=desc&field=last_activity",
        limit.min(50),
        offset
    );
    get_json(app, model_id, &path).await
}

#[allow(dead_code)]
pub async fn list_vault_media(
    app: &AppHandle,
    model_id: &str,
    offset: u32,
    kind: &str,
) -> Result<Value, String> {
    let type_query = match kind {
        "photo" => "&type=photo",
        "video" => "&type=video",
        _ => "",
    };
    let path = format!(
        "/api2/v2/vault/media?limit=40&offset={offset}&field=recent&sort=desc&list=all{type_query}"
    );
    get_json(app, model_id, &path).await
}

#[allow(dead_code)]
pub async fn send_chat_message(
    app: &AppHandle,
    model_id: &str,
    fan_id: &str,
    text: &str,
    price: Option<f64>,
    media_ids: Vec<u64>,
) -> Result<Value, String> {
    let clean_fan = fan_id.trim().trim_start_matches(['u', 'U']);
    if clean_fan.is_empty() || !clean_fan.chars().all(|ch| ch.is_ascii_digit()) {
        return Err("ID de fan inválido".to_owned());
    }
    let path = format!("/api2/v2/chats/{clean_fan}/messages");
    let mut body = json!({
        "text": text,
        "lockedText": price.unwrap_or(0.0) > 0.0,
        "mediaFiles": media_ids,
        "price": price.unwrap_or(0.0),
    });
    if price.unwrap_or(0.0) <= 0.0 {
        if let Some(object) = body.as_object_mut() {
            object.remove("price");
        }
    }
    let raw = request_raw(app, model_id, "POST", &path, Some(body)).await?;
    serde_json::from_str(&raw).map_err(|err| err.to_string())
}

pub async fn list_user_notifications(
    app: &AppHandle,
    model_id: &str,
    notif_type: &str,
    limit: u32,
    from_id: Option<&str>,
) -> Result<Value, String> {
    let kind = notif_type.trim();
    if kind.is_empty() {
        return Err("Tipo de notificación vacío".to_owned());
    }
    let path = match from_id.filter(|value| !value.trim().is_empty()) {
        Some(cursor) => format!(
            "/api2/v2/users/notifications?limit={}&type={}&fromId={}",
            limit.min(50),
            urlencoding::encode(kind),
            urlencoding::encode(cursor.trim())
        ),
        None => format!(
            "/api2/v2/users/notifications?limit={}&type={}",
            limit.min(50),
            urlencoding::encode(kind)
        ),
    };
    get_json(app, model_id, &path).await
}

pub async fn list_subscription_notifications(
    app: &AppHandle,
    model_id: &str,
    limit: u32,
    from_id: Option<&str>,
) -> Result<Value, String> {
    list_user_notifications(app, model_id, "subscribed", limit, from_id).await
}

pub fn page_list(value: &Value) -> Option<&Vec<Value>> {
    value
        .get("list")
        .or_else(|| value.get("users"))
        .or_else(|| value.get("items"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
}

pub fn next_marker(value: &Value) -> Option<String> {
    for key in ["nextMarker", "marker"] {
        if let Some(marker) = value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()) {
            return Some(marker.to_owned());
        }
    }
    None
}

pub fn stop_sync(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("401")
        || lower.contains("403")
        || lower.contains("429")
        || lower.contains("sesión expirada")
        || lower.contains("rate limit")
        || lower.contains("no autenticada")
}
