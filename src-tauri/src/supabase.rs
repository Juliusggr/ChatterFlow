//! Supabase PostgREST persistence layer for CreatorFloww (zero SQLite).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const SUPABASE_URL: &str = "https://rbkjrektvaifegxusrwt.supabase.co";
pub const SUPABASE_PUBLISHABLE_KEY: &str = "sb_publishable_Isv24nfB0NANyUTWQWddmA_doEZBkgn";

/// OnlyFans keeps 20%. Net = gross × this ratio.
pub const ONLYFANS_NET_RATIO: f64 = 0.8;

const MODELS_LIST_TTL: Duration = Duration::from_secs(5);
const BALANCE_TOUCH_INTERVAL: Duration = Duration::from_secs(45);
const PRUNE_INTERVAL_SECS: u64 = 1800;

static PARTITION_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum DbError {
    Auth,
    Http(String),
    Invalid(String),
    NotFound(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Auth => write!(f, "inicia sesión en CreatorFloww para conectar con Supabase"),
            DbError::Http(message) | DbError::Invalid(message) | DbError::NotFound(message) => {
                write!(f, "{message}")
            }
        }
    }
}

impl std::error::Error for DbError {}

#[derive(Debug, Clone)]
struct SessionAuth {
    access_token: String,
    user_id: String,
    role: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionType {
    Sub,
    Tip,
    Post,
    Message,
    Referral,
    Stream,
}

impl TransactionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sub => "sub",
            Self::Tip => "tip",
            Self::Post => "post",
            Self::Message => "message",
            Self::Referral => "referral",
            Self::Stream => "stream",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub of_user_id: Option<String>,
    pub username: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub current_balance: f64,
    pub pending_balance: f64,
    pub last_synced_at: Option<String>,
    pub partition_id: String,
    pub fans_count: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct EarningDraft {
    pub of_transaction_id: String,
    pub fan_of_id: Option<String>,
    pub fan_username: String,
    pub kind: TransactionType,
    pub gross_amount: f64,
    pub net_amount: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EarningStats {
    pub upserted: u64,
    pub linked: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FanListItem {
    pub name: String,
    pub checked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FanLists {
    pub fans: Vec<FanListItem>,
    pub following: Vec<FanListItem>,
}

#[derive(Debug, Clone)]
pub struct FanUpsertOwned {
    pub of_fan_id: String,
    pub username: String,
    pub sub_status: Option<String>,
    pub sub_cost: Option<f64>,
    pub sub_duration: Option<String>,
    pub auto_renewal: Option<bool>,
    pub total_spend: Option<f64>,
    pub lists: Vec<FanListItem>,
    pub messages_summ: Option<f64>,
    pub tips_summ: Option<f64>,
    pub official_spend: bool,
    pub last_spend_at: Option<String>,
    pub highest_spend: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotifyStateRow {
    pub last_seen_message_id: String,
    pub last_message_timestamp: Option<String>,
    pub unread_count: i64,
    pub inbox_seeded: bool,
    pub transactions_seeded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRow {
    pub id: String,
    pub model_id: String,
    pub kind: String,
    pub model_name: String,
    pub model_username: String,
    pub model_avatar: Option<String>,
    pub of_fan_id: String,
    pub fan_name: String,
    #[serde(default, deserialize_with = "de_opt_f64")]
    pub amount: Option<f64>,
    #[serde(default, deserialize_with = "de_opt_f64")]
    pub net_amount: Option<f64>,
    pub preview: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy)]
pub struct PpvUpsertResult {
    pub existed: bool,
    pub prev_purchased: bool,
}

#[derive(Debug, Deserialize)]
struct RemoteModelRow {
    id: String,
    of_user_id: Option<String>,
    #[serde(default)]
    handle: String,
    #[serde(default)]
    name: String,
    avatar_url: Option<String>,
    #[serde(default, deserialize_with = "de_f64")]
    current_balance: f64,
    #[serde(default, deserialize_with = "de_f64")]
    pending_balance: f64,
    last_synced_at: Option<String>,
    #[serde(default)]
    partition_id: String,
    fans_count: Option<i64>,
}

impl From<RemoteModelRow> for Model {
    fn from(row: RemoteModelRow) -> Self {
        let partition_id = if row.partition_id.trim().is_empty() {
            row.id.clone()
        } else {
            row.partition_id
        };
        let of_user_id = row
            .of_user_id
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        Self {
            id: row.id,
            of_user_id,
            username: row.handle,
            display_name: row.name,
            avatar_url: row.avatar_url,
            current_balance: row.current_balance,
            pending_balance: row.pending_balance,
            last_synced_at: row.last_synced_at,
            partition_id,
            fans_count: row.fans_count,
        }
    }
}

pub fn utc_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn unix_seconds(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(trimmed)
        .ok()
        .map(|parsed| parsed.timestamp())
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|naive| naive.and_utc().timestamp())
        })
        .or_else(|| parse_numeric_epoch_seconds(trimmed))
}

fn parse_numeric_epoch_seconds(text: &str) -> Option<i64> {
    let parsed = text.parse::<f64>().ok()?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return None;
    }
    let seconds = if parsed > 100_000_000_000.0 {
        (parsed / 1000.0).floor() as i64
    } else {
        parsed.floor() as i64
    };
    if seconds > 946_684_800 && seconds < 4_102_444_800 {
        Some(seconds)
    } else {
        None
    }
}

pub fn normalize_timestamp_rfc3339(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return Some(parsed.with_timezone(&chrono::Utc).to_rfc3339());
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        return Some(naive.and_utc().to_rfc3339());
    }
    let seconds = parse_numeric_epoch_seconds(trimmed)?;
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0).map(|dt| dt.to_rfc3339())
}

fn next_partition_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let seq = PARTITION_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("of-{millis}-{}-{seq}", std::process::id())
}

#[derive(Default)]
struct ModelsCache {
    by_id: HashMap<String, Model>,
    ordered_ids: Vec<String>,
    list_fetched_at: Option<Instant>,
    balance_touched_at: HashMap<String, Instant>,
}

pub struct SupabaseStore {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    session: RwLock<Option<SessionAuth>>,
    models_cache: RwLock<ModelsCache>,
    seen_publications: Mutex<HashSet<(String, String, bool)>>,
    seen_arrivals: Mutex<HashSet<(String, String)>>,
    fan_labels: RwLock<HashMap<(String, String), (String, String)>>,
    last_prune_epoch: AtomicU64,
}

impl SupabaseStore {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(25))
            .tcp_keepalive(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(16)
            .build()
            .expect("HTTP client for Supabase");
        Self {
            client,
            base_url: SUPABASE_URL.trim_end_matches('/').to_owned(),
            api_key: SUPABASE_PUBLISHABLE_KEY.to_owned(),
            session: RwLock::new(None),
            models_cache: RwLock::new(ModelsCache::default()),
            seen_publications: Mutex::new(HashSet::new()),
            seen_arrivals: Mutex::new(HashSet::new()),
            fan_labels: RwLock::new(HashMap::new()),
            last_prune_epoch: AtomicU64::new(0),
        }
    }

    /// Updates auth session and returns `true` if the authenticated user changed or logged out.
    pub fn set_session(
        &self,
        access_token: Option<String>,
        user_id: Option<String>,
        role: Option<String>,
    ) -> bool {
        let next_user = user_id.unwrap_or_default();
        let next_role = role.unwrap_or_default();
        let mut user_changed = false;
        {
            let mut slot = self
                .session
                .write()
                .unwrap_or_else(|poison| poison.into_inner());
            let prev_user = slot.as_ref().map(|auth| auth.user_id.as_str()).unwrap_or("");
            match access_token.filter(|token| !token.trim().is_empty()) {
                Some(token) => {
                    if prev_user != next_user {
                        user_changed = true;
                    }
                    *slot = Some(SessionAuth {
                        access_token: token,
                        user_id: next_user,
                        role: next_role,
                    });
                }
                None => {
                    if !prev_user.is_empty() {
                        user_changed = true;
                    }
                    *slot = None;
                }
            }
        }
        if user_changed {
            self.clear_caches();
        }
        user_changed
    }

    pub fn clear_caches(&self) {
        *self
            .models_cache
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = ModelsCache::default();
        self.seen_publications
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
        self.seen_arrivals
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
        self.fan_labels
            .write()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
    }

    pub fn has_session(&self) -> bool {
        self.session
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_some()
    }

    pub fn active_dropper_id(&self) -> Option<String> {
        let slot = self
            .session
            .read()
            .unwrap_or_else(|poison| poison.into_inner());
        let auth = slot.as_ref()?;
        if auth.role == "dropper" && !auth.user_id.trim().is_empty() {
            Some(auth.user_id.clone())
        } else {
            None
        }
    }

    fn token(&self) -> Result<String, DbError> {
        self.session
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .map(|auth| auth.access_token.clone())
            .ok_or(DbError::Auth)
    }

    fn request(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder, DbError> {
        let token = self.token()?;
        let url = format!("{}/rest/v1/{}", self.base_url, path.trim_start_matches('/'));
        Ok(self
            .client
            .request(method, url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json"))
    }

    async fn send_json<T: for<'de> Deserialize<'de>>(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<T, DbError> {
        let response = builder
            .send()
            .await
            .map_err(|err| DbError::Http(format!("error de red con Supabase: {err}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| DbError::Http(format!("error leyendo respuesta de Supabase: {err}")))?;
        if !status.is_success() {
            return Err(parse_postgrest_error(status.as_u16(), &text));
        }
        serde_json::from_str(&text)
            .map_err(|err| DbError::Http(format!("respuesta JSON inesperada de Supabase: {err}")))
    }

    async fn send_empty(&self, builder: reqwest::RequestBuilder) -> Result<(), DbError> {
        let response = builder
            .send()
            .await
            .map_err(|err| DbError::Http(format!("error de red con Supabase: {err}")))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(parse_postgrest_error(status.as_u16(), &text));
        }
        Ok(())
    }

    fn cache_put_model(&self, model: Model, invalidate_list: bool) {
        let mut cache = self
            .models_cache
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        cache.by_id.insert(model.id.clone(), model);
        if invalidate_list {
            cache.list_fetched_at = None;
        }
    }

    pub async fn list_models(&self) -> Result<Vec<Model>, DbError> {
        {
            let cache = self
                .models_cache
                .read()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(fetched_at) = cache.list_fetched_at {
                if fetched_at.elapsed() < MODELS_LIST_TTL {
                    let cached: Vec<Model> = cache
                        .ordered_ids
                        .iter()
                        .filter_map(|id| cache.by_id.get(id).cloned())
                        .collect();
                    if cached.len() == cache.ordered_ids.len() {
                        return Ok(cached);
                    }
                }
            }
        }

        let req = self.request(Method::GET, "models")?.query(&[
            (
                "select",
                "id,of_user_id,handle,name,avatar_url,current_balance,pending_balance,last_synced_at,partition_id,fans_count",
            ),
            ("order", "name.asc,handle.asc,id.asc"),
        ]);
        let rows: Vec<RemoteModelRow> = self.send_json(req).await?;
        let models: Vec<Model> = rows.into_iter().map(Model::from).collect();
        {
            let mut cache = self
                .models_cache
                .write()
                .unwrap_or_else(|poison| poison.into_inner());
            cache.ordered_ids = models.iter().map(|m| m.id.clone()).collect();
            for model in &models {
                cache.by_id.insert(model.id.clone(), model.clone());
            }
            cache.list_fetched_at = Some(Instant::now());
        }
        Ok(models)
    }

    pub async fn get_model(&self, id: &str) -> Result<Model, DbError> {
        if let Some(cached) = self
            .models_cache
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .by_id
            .get(id)
            .cloned()
        {
            return Ok(cached);
        }
        self.fetch_model_uncached(id).await
    }

    pub async fn fetch_model_uncached(&self, id: &str) -> Result<Model, DbError> {
        let id_eq = format!("eq.{id}");
        let req = self.request(Method::GET, "models")?.query(&[
            (
                "select",
                "id,of_user_id,handle,name,avatar_url,current_balance,pending_balance,last_synced_at,partition_id,fans_count",
            ),
            ("id", id_eq.as_str()),
            ("limit", "1"),
        ]);
        let mut rows: Vec<RemoteModelRow> = self.send_json(req).await?;
        let model = rows
            .pop()
            .map(Model::from)
            .ok_or_else(|| DbError::NotFound(format!("no existe la modelo {id}")))?;
        self.cache_put_model(model.clone(), false);
        Ok(model)
    }

    pub async fn create_model_partition(&self) -> Result<Model, DbError> {
        let partition_id = next_partition_id();
        let external_key = format!("partition:{partition_id}");
        let body = json!({
            "name": "",
            "handle": "",
            "partition_id": partition_id,
            "external_key": external_key,
            "session_data": { "partition_id": partition_id }
        });
        let req = self
            .request(Method::POST, "models")?
            .header("Prefer", "return=representation")
            .query(&[(
                "select",
                "id,of_user_id,handle,name,avatar_url,current_balance,pending_balance,last_synced_at,partition_id,fans_count",
            )])
            .json(&body);
        let mut rows: Vec<RemoteModelRow> = self.send_json(req).await?;
        let created = rows
            .pop()
            .map(Model::from)
            .ok_or_else(|| DbError::Http("Supabase no devolvió la modelo creada".to_owned()))?;
        self.cache_put_model(created.clone(), true);
        Ok(created)
    }

    pub async fn find_reusable_empty_model(
        &self,
        open_model_ids: &[String],
    ) -> Result<Option<Model>, DbError> {
        let models = self.list_models().await?;
        Ok(models.into_iter().rev().find(|model| {
            model.of_user_id.is_none()
                && model.username.trim().is_empty()
                && model.display_name.trim().is_empty()
                && !open_model_ids.iter().any(|open| open == &model.id)
        }))
    }

    pub async fn delete_model(&self, id: &str) -> Result<(), DbError> {
        let id_eq = format!("eq.{id}");
        let req = self
            .request(Method::DELETE, "models")?
            .query(&[("id", id_eq.as_str())]);
        self.send_empty(req).await?;
        let mut cache = self
            .models_cache
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        cache.by_id.remove(id);
        cache.ordered_ids.retain(|item| item != id);
        cache.list_fetched_at = None;
        Ok(())
    }

    pub async fn save_onlyfans_profile(
        &self,
        model_id: &str,
        of_user_id: &str,
        username: &str,
        display_name: &str,
        avatar_url: Option<&str>,
        fans_count: Option<i64>,
    ) -> Result<(Model, bool), DbError> {
        let of_user_id = of_user_id.trim();
        if of_user_id.is_empty() {
            return Err(DbError::Invalid(
                "OnlyFans no devolvió el id de la cuenta".to_owned(),
            ));
        }

        // Fast skip if the cached model already has identical profile fields and was synced recently.
        if let Some(cached) = self
            .models_cache
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .by_id
            .get(model_id)
            .cloned()
        {
            let same_user = cached.of_user_id.as_deref() == Some(of_user_id);
            let same_handle = username.trim().is_empty() || cached.username == username.trim();
            let same_name =
                display_name.trim().is_empty() || cached.display_name == display_name.trim();
            let same_avatar = avatar_url
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .is_none_or(|u| cached.avatar_url.as_deref() == Some(u));
            let same_fans = fans_count
                .map(|v| v.max(0))
                .is_none_or(|v| cached.fans_count == Some(v));
            if same_user && same_handle && same_name && same_avatar && same_fans {
                return Ok((cached, false));
            }
        }

        let of_eq = format!("eq.{of_user_id}");
        let id_neq = format!("neq.{model_id}");
        let check = self.request(Method::GET, "models")?.query(&[
            ("select", "id"),
            ("of_user_id", of_eq.as_str()),
            ("id", id_neq.as_str()),
            ("limit", "1"),
        ]);
        let owners: Vec<Value> = self.send_json(check).await?;
        if !owners.is_empty() {
            return Err(DbError::Invalid(
                "esa cuenta de OnlyFans ya está agregada".to_owned(),
            ));
        }

        let mut patch = serde_json::Map::new();
        patch.insert("of_user_id".to_owned(), json!(of_user_id));
        if !username.trim().is_empty() {
            patch.insert("handle".to_owned(), json!(username.trim()));
        }
        if !display_name.trim().is_empty() {
            patch.insert("name".to_owned(), json!(display_name.trim()));
        }
        if let Some(url) = avatar_url.map(str::trim).filter(|url| !url.is_empty()) {
            patch.insert("avatar_url".to_owned(), json!(url));
        }
        if let Some(fans) = fans_count {
            patch.insert("fans_count".to_owned(), json!(fans.max(0)));
        }
        patch.insert("last_synced_at".to_owned(), json!(utc_now()));

        let id_filter = format!("eq.{model_id}");
        let req = self
            .request(Method::PATCH, "models")?
            .header("Prefer", "return=representation")
            .query(&[
                ("id", id_filter.as_str()),
                (
                    "select",
                    "id,of_user_id,handle,name,avatar_url,current_balance,pending_balance,last_synced_at,partition_id,fans_count",
                ),
            ])
            .json(&Value::Object(patch));
        let mut rows: Vec<RemoteModelRow> = self.send_json(req).await?;
        let updated = rows
            .pop()
            .map(Model::from)
            .ok_or_else(|| DbError::NotFound(format!("no existe la modelo {model_id}")))?;
        self.cache_put_model(updated.clone(), true);
        Ok((updated, true))
    }

    pub async fn adopt_partition_into_existing_model(
        &self,
        placeholder_id: &str,
        of_user_id: &str,
        username: &str,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<Option<(Model, String)>, DbError> {
        let req = self
            .request(Method::POST, "rpc/adopt_model_partition")?
            .json(&json!({
                "p_placeholder_id": placeholder_id,
                "p_of_user_id": of_user_id.trim(),
                "p_username": username.trim(),
                "p_display_name": display_name.trim(),
                "p_avatar_url": avatar_url,
            }));
        let value: Value = self.send_json(req).await?;
        if value.is_null() {
            return Ok(None);
        }
        let Some(model_val) = value.get("model") else {
            return Ok(None);
        };
        let row: RemoteModelRow = serde_json::from_value(model_val.clone())
            .map_err(|err| DbError::Http(err.to_string()))?;
        let old_partition = value
            .get("old_partition")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let adopted = Model::from(row);
        {
            let mut cache = self
                .models_cache
                .write()
                .unwrap_or_else(|poison| poison.into_inner());
            cache.by_id.remove(placeholder_id);
            cache.ordered_ids.retain(|id| id != placeholder_id);
            cache.by_id.insert(adopted.id.clone(), adopted.clone());
            cache.list_fetched_at = None;
        }
        Ok(Some((adopted, old_partition)))
    }

    pub async fn save_onlyfans_balances(
        &self,
        model_id: &str,
        current: Option<f64>,
        pending: Option<f64>,
    ) -> Result<bool, DbError> {
        let cur_clean = current.filter(|v| v.is_finite()).map(round2);
        let pend_clean = pending.filter(|v| v.is_finite()).map(round2);
        if cur_clean.is_none() && pend_clean.is_none() {
            return Ok(false);
        }

        {
            let cache = self
                .models_cache
                .read()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(existing) = cache.by_id.get(model_id) {
                let same_cur = cur_clean.is_none_or(|v| (existing.current_balance - v).abs() < 0.005);
                let same_pend =
                    pend_clean.is_none_or(|v| (existing.pending_balance - v).abs() < 0.005);
                let recently_touched = cache
                    .balance_touched_at
                    .get(model_id)
                    .is_some_and(|t| t.elapsed() < BALANCE_TOUCH_INTERVAL);
                if same_cur && same_pend && recently_touched {
                    return Ok(false);
                }
            }
        }

        let now = utc_now();
        let mut patch = serde_json::Map::new();
        if let Some(value) = cur_clean {
            patch.insert("current_balance".to_owned(), json!(value));
        }
        if let Some(value) = pend_clean {
            patch.insert("pending_balance".to_owned(), json!(value));
        }
        patch.insert("last_synced_at".to_owned(), json!(now));
        let id_eq = format!("eq.{model_id}");
        let req = self
            .request(Method::PATCH, "models")?
            .header("Prefer", "return=minimal")
            .query(&[("id", id_eq.as_str())])
            .json(&Value::Object(patch));
        self.send_empty(req).await?;

        let mut changed = false;
        let mut cache = self
            .models_cache
            .write()
            .unwrap_or_else(|poison| poison.into_inner());
        cache
            .balance_touched_at
            .insert(model_id.to_owned(), Instant::now());
        if let Some(existing) = cache.by_id.get_mut(model_id) {
            if let Some(v) = cur_clean {
                if (existing.current_balance - v).abs() >= 0.005 {
                    changed = true;
                }
                existing.current_balance = v;
            }
            if let Some(v) = pend_clean {
                if (existing.pending_balance - v).abs() >= 0.005 {
                    changed = true;
                }
                existing.pending_balance = v;
            }
            existing.last_synced_at = Some(now);
        } else {
            changed = true;
            cache.list_fetched_at = None;
        }
        Ok(changed)
    }

    pub async fn record_publications_batch(
        &self,
        model_id: &str,
        items: &[(String, String)],
        author_user_id: Option<&str>,
    ) -> Result<bool, DbError> {
        if items.is_empty() {
            return Ok(false);
        }
        let has_author = author_user_id.is_some_and(|a| !a.trim().is_empty());
        let mut pending = Vec::new();
        {
            let seen = self
                .seen_publications
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            for (post_id, created_at) in items {
                let clean_id = post_id.trim();
                if clean_id.is_empty() {
                    continue;
                }
                let already = seen.contains(&(model_id.to_owned(), clean_id.to_owned(), true))
                    || (!has_author
                        && seen.contains(&(model_id.to_owned(), clean_id.to_owned(), false)));
                if !already {
                    let norm = normalize_timestamp_rfc3339(created_at).unwrap_or_else(utc_now);
                    pending.push((clean_id.to_owned(), norm));
                }
            }
        }
        if pending.is_empty() {
            return Ok(false);
        }

        let json_items: Vec<Value> = pending
            .iter()
            .map(|(post_id, created_at)| {
                json!({
                    "of_post_id": post_id,
                    "created_at": created_at,
                })
            })
            .collect();

        let batch_req = self
            .request(Method::POST, "rpc/record_publications_batch")?
            .json(&json!({
                "p_model_id": model_id,
                "p_items": json_items,
                "p_author_user_id": author_user_id,
            }));

        let changed = match self.send_json::<Value>(batch_req).await {
            Ok(val) => val.as_i64().unwrap_or(0) > 0 || val.as_bool().unwrap_or(false),
            Err(err) if is_missing_rpc(&err) => {
                let mut any = false;
                for (post_id, created_at) in &pending {
                    let req = self
                        .request(Method::POST, "rpc/record_publication")?
                        .json(&json!({
                            "p_model_id": model_id,
                            "p_of_post_id": post_id,
                            "p_created_at": created_at,
                            "p_author_user_id": author_user_id,
                        }));
                    if self
                        .send_json::<Value>(req)
                        .await
                        .map(|v| v.as_bool().unwrap_or(true))
                        .unwrap_or(false)
                    {
                        any = true;
                    }
                }
                any
            }
            Err(err) => return Err(err),
        };

        {
            let mut seen = self
                .seen_publications
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            for (post_id, _) in pending {
                seen.insert((model_id.to_owned(), post_id, has_author));
            }
        }
        Ok(changed)
    }

    pub async fn record_fan_arrivals_batch(
        &self,
        model_id: &str,
        items: &[(String, String)],
    ) -> Result<bool, DbError> {
        if items.is_empty() {
            return Ok(false);
        }
        let mut pending = Vec::new();
        let mut local_dedup = HashSet::new();
        {
            let seen = self
                .seen_arrivals
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            for (fan_id, arrived_at) in items {
                let clean_id = fan_id.trim();
                if clean_id.is_empty() || !local_dedup.insert(clean_id.to_owned()) {
                    continue;
                }
                if !seen.contains(&(model_id.to_owned(), clean_id.to_owned())) {
                    let norm = normalize_timestamp_rfc3339(arrived_at).unwrap_or_else(utc_now);
                    pending.push((clean_id.to_owned(), norm));
                }
            }
        }
        if pending.is_empty() {
            return Ok(false);
        }

        let json_items: Vec<Value> = pending
            .iter()
            .map(|(fan_id, arrived_at)| {
                json!({
                    "of_fan_id": fan_id,
                    "arrived_at": arrived_at,
                })
            })
            .collect();

        let batch_req = self
            .request(Method::POST, "rpc/record_fan_arrivals_batch")?
            .json(&json!({
                "p_model_id": model_id,
                "p_items": json_items,
            }));

        let inserted = match self.send_json::<Value>(batch_req).await {
            Ok(val) => val.as_i64().unwrap_or(0) > 0 || val.as_bool().unwrap_or(false),
            Err(err) if is_missing_rpc(&err) => {
                let mut any = false;
                for (fan_id, arrived_at) in &pending {
                    let req = self
                        .request(Method::POST, "rpc/record_fan_arrival")?
                        .json(&json!({
                            "p_model_id": model_id,
                            "p_of_fan_id": fan_id,
                            "p_arrived_at": arrived_at,
                        }));
                    if self
                        .send_json::<Value>(req)
                        .await
                        .map(|v| v.as_bool().unwrap_or(false))
                        .unwrap_or(false)
                    {
                        any = true;
                    }
                }
                any
            }
            Err(err) => return Err(err),
        };

        {
            let mut seen = self
                .seen_arrivals
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            for (fan_id, _) in pending {
                seen.insert((model_id.to_owned(), fan_id));
            }
        }
        Ok(inserted)
    }

    pub async fn upsert_fans_batch(
        &self,
        model_id: &str,
        fans: &[FanUpsertOwned],
    ) -> Result<(), DbError> {
        if fans.is_empty() {
            return Ok(());
        }
        let mut by_id: HashMap<String, &FanUpsertOwned> = HashMap::with_capacity(fans.len());
        for fan in fans {
            let clean = fan.of_fan_id.trim();
            if !clean.is_empty() {
                by_id.insert(clean.to_owned(), fan);
            }
        }
        if by_id.is_empty() {
            return Ok(());
        }

        // Update in-memory fan username cache so notifications resolve usernames in 0ms.
        {
            let mut labels = self
                .fan_labels
                .write()
                .unwrap_or_else(|poison| poison.into_inner());
            for (fan_id, fan) in &by_id {
                let uname = fan.username.trim();
                if !uname.is_empty() {
                    let entry = labels
                        .entry((model_id.to_owned(), fan_id.clone()))
                        .or_insert_with(|| (String::new(), String::new()));
                    entry.1 = uname.to_owned();
                }
            }
        }

        let rpc_items: Vec<Value> = by_id
            .iter()
            .map(|(of_fan_id, fan)| {
                let lists_json = if fan.lists.is_empty() {
                    Value::Null
                } else {
                    json!(FanLists {
                        fans: fan.lists.clone(),
                        following: Vec::new(),
                    })
                };
                json!({
                    "of_fan_id": of_fan_id,
                    "username": fan.username.trim(),
                    "sub_status": fan.sub_status,
                    "sub_cost": fan.sub_cost.filter(|v| v.is_finite()).map(round2),
                    "sub_duration": fan.sub_duration,
                    "auto_renewal": fan.auto_renewal,
                    "total_spend": fan.total_spend.filter(|v| v.is_finite()).map(round2),
                    "ppv_total": fan.messages_summ.filter(|v| v.is_finite()).map(round2),
                    "tip_total": fan.tips_summ.filter(|v| v.is_finite()).map(round2),
                    "official_spend": fan.official_spend,
                    "last_spend_at": fan.last_spend_at.as_deref().and_then(normalize_timestamp_rfc3339),
                    "highest_spend": fan.highest_spend.filter(|v| v.is_finite()).map(round2),
                    "lists_json": lists_json,
                })
            })
            .collect();

        for chunk in rpc_items.chunks(200) {
            let req = self
                .request(Method::POST, "rpc/upsert_fans_batch")?
                .json(&json!({
                    "p_model_id": model_id,
                    "p_items": chunk,
                }));
            match self.send_empty(req).await {
                Ok(()) => {}
                Err(err) if is_missing_rpc(&err) => {
                    let now = utc_now();
                    let fallback_rows: Vec<Value> = by_id
                        .iter()
                        .map(|(of_fan_id, fan)| {
                            json!({
                                "model_id": model_id,
                                "of_fan_id": of_fan_id,
                                "username": fan.username.trim(),
                                "sub_status": fan.sub_status,
                                "sub_cost": fan.sub_cost.filter(|v| v.is_finite()).map(round2),
                                "sub_duration": fan.sub_duration,
                                "auto_renewal": fan.auto_renewal,
                                "total_spend": fan.total_spend.filter(|v| v.is_finite()).map(round2).unwrap_or(0.0),
                                "ppv_total": fan.messages_summ.filter(|v| v.is_finite()).map(round2).unwrap_or(0.0),
                                "tip_total": fan.tips_summ.filter(|v| v.is_finite()).map(round2).unwrap_or(0.0),
                                "official_spend": fan.official_spend,
                                "last_spend_at": fan.last_spend_at.as_deref().and_then(normalize_timestamp_rfc3339),
                                "highest_spend": fan.highest_spend.filter(|v| v.is_finite()).map(round2).unwrap_or(0.0),
                                "lists_json": FanLists {
                                    fans: fan.lists.clone(),
                                    following: Vec::new(),
                                },
                                "updated_at": now,
                            })
                        })
                        .collect();
                    for fb_chunk in fallback_rows.chunks(200) {
                        let fb_req = self
                            .request(Method::POST, "fans")?
                            .header("Prefer", "resolution=merge-duplicates,return=minimal")
                            .query(&[("on_conflict", "model_id,of_fan_id")])
                            .json(&fb_chunk);
                        self.send_empty(fb_req).await?;
                    }
                    break;
                }
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    pub async fn refresh_fan_spend_peaks(&self, model_id: &str) -> Result<(), DbError> {
        let req = self
            .request(Method::POST, "rpc/refresh_fan_spend_peaks")?
            .json(&json!({ "p_model_id": model_id }));
        match self.send_empty(req).await {
            Ok(()) => Ok(()),
            Err(err) if is_missing_rpc(&err) => Ok(()),
            Err(err) => Err(err),
        }
    }

    pub async fn delete_chart_span(
        &self,
        model_id: &str,
        kind: &str,
        start_unix: i64,
        end_unix: i64,
    ) -> Result<(), DbError> {
        let Some(start_iso) = chrono::DateTime::<chrono::Utc>::from_timestamp(start_unix, 0)
            .map(|dt| dt.to_rfc3339())
        else {
            return Ok(());
        };
        let Some(end_iso) = chrono::DateTime::<chrono::Utc>::from_timestamp(end_unix, 0)
            .map(|dt| dt.to_rfc3339())
        else {
            return Ok(());
        };
        let req = self
            .request(Method::DELETE, "model_transactions")?
            .query(&[
                ("model_id", format!("eq.{model_id}")),
                ("type", format!("eq.{kind}")),
                ("of_transaction_id", format!("like.chart:{model_id}:*")),
                ("created_at", format!("gte.{start_iso}")),
                ("created_at", format!("lt.{end_iso}")),
            ]);
        self.send_empty(req).await
    }

    pub async fn upsert_earnings_with_new(
        &self,
        model_id: &str,
        drafts: &[EarningDraft],
    ) -> Result<(EarningStats, Vec<EarningDraft>), DbError> {
        if drafts.is_empty() {
            return Ok((EarningStats { upserted: 0, linked: 0 }, Vec::new()));
        }
        let mut unique = Vec::new();
        let mut seen = HashSet::new();
        for draft in drafts {
            let tx_id = draft.of_transaction_id.trim();
            if tx_id.is_empty() || !seen.insert(tx_id.to_owned()) {
                continue;
            }
            unique.push(draft.clone());
        }
        if unique.is_empty() {
            return Ok((EarningStats { upserted: 0, linked: 0 }, Vec::new()));
        }

        // Chart synthetic rows never emit sale notifications, so skip the pre-check SELECT for pure chart batches!
        let has_non_chart = unique
            .iter()
            .any(|item| !item.of_transaction_id.starts_with("chart:"));
        let mut existing_ids = HashSet::new();
        if has_non_chart {
            let non_chart: Vec<&EarningDraft> = unique
                .iter()
                .filter(|item| !item.of_transaction_id.starts_with("chart:"))
                .collect();
            for chunk in non_chart.chunks(100) {
                let filter_list = chunk
                    .iter()
                    .map(|item| format!("\"{}\"", item.of_transaction_id.replace('"', "")))
                    .collect::<Vec<_>>()
                    .join(",");
                let req = self.request(Method::GET, "model_transactions")?.query(&[
                    ("select", "of_transaction_id".to_owned()),
                    ("of_transaction_id", format!("in.({filter_list})")),
                ]);
                let rows: Vec<Value> = self.send_json(req).await?;
                for row in rows {
                    if let Some(id) = row.get("of_transaction_id").and_then(Value::as_str) {
                        existing_ids.insert(id.to_owned());
                    }
                }
            }
        }

        let mut newly_inserted = Vec::new();
        let mut payload = Vec::with_capacity(unique.len());
        for draft in &unique {
            if !draft.of_transaction_id.starts_with("chart:")
                && !existing_ids.contains(&draft.of_transaction_id)
            {
                newly_inserted.push(draft.clone());
            }
            let created_at =
                normalize_timestamp_rfc3339(&draft.created_at).unwrap_or_else(utc_now);
            payload.push(json!({
                "of_transaction_id": draft.of_transaction_id,
                "model_id": model_id,
                "of_fan_id": draft.fan_of_id,
                "fan_username": draft.fan_username,
                "type": draft.kind.as_str(),
                "gross_amount": round2(draft.gross_amount),
                "net_amount": round2(draft.net_amount),
                "created_at": created_at,
            }));
        }

        for chunk in payload.chunks(250) {
            let req = self
                .request(Method::POST, "model_transactions")?
                .header("Prefer", "resolution=merge-duplicates,return=minimal")
                .query(&[("on_conflict", "of_transaction_id")])
                .json(&chunk);
            self.send_empty(req).await?;
        }
        if unique.iter().any(|item| {
            item.fan_of_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty())
        }) {
            let _ = self.refresh_fan_spend_peaks(model_id).await;
        }

        Ok((
            EarningStats {
                upserted: unique.len() as u64,
                linked: newly_inserted.len() as u64,
            },
            newly_inserted,
        ))
    }

    pub async fn lookup_fan_label(
        &self,
        model_id: &str,
        of_fan_id: &str,
    ) -> Result<Option<(String, String)>, DbError> {
        let key = (model_id.to_owned(), of_fan_id.to_owned());
        if let Some(cached) = self
            .fan_labels
            .read()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&key)
            .cloned()
        {
            return Ok(Some(cached));
        }

        let req = self.request(Method::GET, "fans")?.query(&[
            ("select", "nickname,username".to_owned()),
            ("model_id", format!("eq.{model_id}")),
            ("of_fan_id", format!("eq.{of_fan_id}")),
            ("limit", "1".to_owned()),
        ]);
        let rows: Vec<Value> = self.send_json(req).await?;
        let found = rows.first().map(|row| {
            (
                row.get("nickname").and_then(Value::as_str).unwrap_or("").to_owned(),
                row.get("username").and_then(Value::as_str).unwrap_or("").to_owned(),
            )
        });
        if let Some(label) = &found {
            self.fan_labels
                .write()
                .unwrap_or_else(|poison| poison.into_inner())
                .insert(key, label.clone());
        }
        Ok(found)
    }

    pub async fn insert_chat_tips_batch(
        &self,
        model_id: &str,
        tips: &[(String, String, f64, String)],
    ) -> Result<HashSet<String>, DbError> {
        if tips.is_empty() {
            return Ok(HashSet::new());
        }
        let mut seen = HashSet::new();
        let mut payload = Vec::with_capacity(tips.len());
        for (of_fan_id, of_message_id, amount, created_at) in tips {
            if !seen.insert(of_message_id.clone()) {
                continue;
            }
            let created_norm = normalize_timestamp_rfc3339(created_at).unwrap_or_else(utc_now);
            payload.push(json!({
                "model_id": model_id,
                "of_fan_id": of_fan_id,
                "of_message_id": of_message_id,
                "amount": round2(*amount),
                "created_at": created_norm,
            }));
        }
        let req = self
            .request(Method::POST, "chat_tips")?
            .header("Prefer", "resolution=ignore-duplicates,return=representation")
            .query(&[
                ("on_conflict", "model_id,of_message_id"),
                ("select", "of_message_id"),
            ])
            .json(&payload);
        let rows: Vec<Value> = self.send_json(req).await?;
        let mut inserted = HashSet::new();
        for row in rows {
            if let Some(id) = row.get("of_message_id").and_then(Value::as_str) {
                inserted.insert(id.to_owned());
            }
        }
        let _ = self.refresh_fan_spend_peaks(model_id).await;
        Ok(inserted)
    }

    pub async fn upsert_ppv_timeline_batch(
        &self,
        model_id: &str,
        items: &[(String, String, f64, bool, String)],
    ) -> Result<HashMap<String, PpvUpsertResult>, DbError> {
        if items.is_empty() {
            return Ok(HashMap::new());
        }
        let mut dedup: HashMap<String, &(String, String, f64, bool, String)> =
            HashMap::with_capacity(items.len());
        for item in items {
            dedup.insert(item.1.clone(), item);
        }

        let msg_ids: Vec<&String> = dedup.keys().collect();
        let mut existing_map: HashMap<String, bool> = HashMap::new();
        for chunk in msg_ids.chunks(100) {
            let filter_list = chunk
                .iter()
                .map(|id| format!("\"{}\"", id.replace('"', "")))
                .collect::<Vec<_>>()
                .join(",");
            let check = self.request(Method::GET, "ppv_timeline")?.query(&[
                ("select", "of_message_id,purchased".to_owned()),
                ("model_id", format!("eq.{model_id}")),
                ("of_message_id", format!("in.({filter_list})")),
            ]);
            let rows: Vec<Value> = self.send_json(check).await?;
            for row in rows {
                if let Some(msg_id) = row.get("of_message_id").and_then(Value::as_str) {
                    let prev_purchased = row
                        .get("purchased")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    existing_map.insert(msg_id.to_owned(), prev_purchased);
                }
            }
        }

        let mut results = HashMap::with_capacity(dedup.len());
        let mut payload = Vec::with_capacity(dedup.len());
        for (msg_id, (fan_id, _, price, purchased, created_at)) in &dedup {
            let prev = existing_map.get(msg_id).copied();
            let existed = prev.is_some();
            let prev_purchased = prev.unwrap_or(false);
            let next_purchased = prev_purchased || *purchased;
            let created_norm = normalize_timestamp_rfc3339(created_at).unwrap_or_else(utc_now);
            payload.push(json!({
                "model_id": model_id,
                "of_fan_id": fan_id,
                "of_message_id": msg_id,
                "price": round2(*price),
                "purchased": next_purchased,
                "created_at": created_norm,
            }));
            results.insert(
                msg_id.clone(),
                PpvUpsertResult {
                    existed,
                    prev_purchased,
                },
            );
        }

        for chunk in payload.chunks(200) {
            let req = self
                .request(Method::POST, "ppv_timeline")?
                .header("Prefer", "resolution=merge-duplicates,return=minimal")
                .query(&[("on_conflict", "model_id,of_message_id")])
                .json(&chunk);
            self.send_empty(req).await?;
        }
        let _ = self.refresh_fan_spend_peaks(model_id).await;

        Ok(results)
    }

    pub async fn load_notify_state(&self, model_id: &str) -> Result<NotifyStateRow, DbError> {
        let req = self.request(Method::GET, "account_notify_state")?.query(&[
            (
                "select",
                "last_seen_message_id,last_message_timestamp,unread_count,inbox_seeded,transactions_seeded".to_owned(),
            ),
            ("model_id", format!("eq.{model_id}")),
            ("limit", "1".to_owned()),
        ]);
        let rows: Vec<Value> = self.send_json(req).await?;
        let Some(row) = rows.first() else {
            return Ok(NotifyStateRow::default());
        };
        Ok(NotifyStateRow {
            last_seen_message_id: row
                .get("last_seen_message_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            last_message_timestamp: row
                .get("last_message_timestamp")
                .and_then(Value::as_str)
                .map(str::to_owned),
            unread_count: row.get("unread_count").and_then(Value::as_i64).unwrap_or(0),
            inbox_seeded: row
                .get("inbox_seeded")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            transactions_seeded: row
                .get("transactions_seeded")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    pub async fn save_notify_state(
        &self,
        model_id: &str,
        row: &NotifyStateRow,
    ) -> Result<(), DbError> {
        let req = self
            .request(Method::POST, "account_notify_state")?
            .header("Prefer", "resolution=merge-duplicates,return=minimal")
            .query(&[("on_conflict", "model_id")])
            .json(&json!({
                "model_id": model_id,
                "last_seen_message_id": row.last_seen_message_id,
                "last_message_timestamp": row.last_message_timestamp,
                "unread_count": row.unread_count,
                "inbox_seeded": row.inbox_seeded,
                "transactions_seeded": row.transactions_seeded,
            }));
        self.send_empty(req).await
    }

    pub async fn claim_notified_event(
        &self,
        model_id: &str,
        event_key: &str,
        now_iso: &str,
    ) -> Result<bool, DbError> {
        if event_key.trim().is_empty() {
            return Ok(false);
        }
        let req = self
            .request(Method::POST, "notified_events")?
            .header("Prefer", "resolution=ignore-duplicates,return=representation")
            .query(&[("on_conflict", "model_id,event_key"), ("select", "event_key")])
            .json(&json!({
                "model_id": model_id,
                "event_key": event_key,
                "notified_at": now_iso,
            }));
        let rows: Vec<Value> = self.send_json(req).await?;
        Ok(!rows.is_empty())
    }

    pub async fn claim_notified_events_batch(
        &self,
        model_id: &str,
        event_keys: &[String],
        now_iso: &str,
    ) -> Result<(), DbError> {
        let mut unique = HashSet::new();
        let mut payload = Vec::new();
        for key in event_keys {
            let clean = key.trim();
            if !clean.is_empty() && unique.insert(clean.to_owned()) {
                payload.push(json!({
                    "model_id": model_id,
                    "event_key": clean,
                    "notified_at": now_iso,
                }));
            }
        }
        if payload.is_empty() {
            return Ok(());
        }
        for chunk in payload.chunks(250) {
            let req = self
                .request(Method::POST, "notified_events")?
                .header("Prefer", "resolution=ignore-duplicates,return=minimal")
                .query(&[("on_conflict", "model_id,event_key")])
                .json(&chunk);
            let _ = self.send_empty(req).await;
        }
        Ok(())
    }

    pub async fn prune_notified_events_throttled(&self, cutoff_iso: &str) {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let prev = self.last_prune_epoch.load(Ordering::Relaxed);
        if now_secs.saturating_sub(prev) < PRUNE_INTERVAL_SECS {
            return;
        }
        self.last_prune_epoch.store(now_secs, Ordering::Relaxed);
        if let Ok(req) = self.request(Method::DELETE, "notified_events") {
            let req = req.query(&[("notified_at", format!("lt.{cutoff_iso}"))]);
            let _ = self.send_empty(req).await;
        }
    }

    pub async fn insert_notification_history_batch(
        &self,
        items: &[NotificationRow],
    ) -> Result<(), DbError> {
        if items.is_empty() {
            return Ok(());
        }
        let req = self
            .request(Method::POST, "notification_history")?
            .header("Prefer", "resolution=merge-duplicates,return=minimal")
            .query(&[("on_conflict", "id")])
            .json(items);
        self.send_empty(req).await
    }

    pub async fn list_recent_notifications(
        &self,
        limit: usize,
    ) -> Result<Vec<NotificationRow>, DbError> {
        let req = self.request(Method::GET, "notification_history")?.query(&[
            (
                "select",
                "id,model_id,kind,model_name,model_username,model_avatar,of_fan_id,fan_name,amount,net_amount,preview,created_at".to_owned(),
            ),
            ("order", "created_at.desc,id.desc".to_owned()),
            ("limit", limit.to_string()),
        ]);
        self.send_json(req).await
    }

    pub async fn clear_notification_history(&self) -> Result<(), DbError> {
        let req = self
            .request(Method::DELETE, "notification_history")?
            .query(&[("id", "neq.")]);
        self.send_empty(req).await
    }

    pub async fn dismiss_notification(&self, id: &str) -> Result<(), DbError> {
        let req = self
            .request(Method::DELETE, "notification_history")?
            .query(&[("id", format!("eq.{id}"))]);
        self.send_empty(req).await
    }

    pub async fn unread_counts(&self) -> Result<HashMap<String, u32>, DbError> {
        let req = self.request(Method::GET, "account_notify_state")?.query(&[
            ("select", "model_id,unread_count".to_owned()),
            ("unread_count", "gt.0".to_owned()),
        ]);
        let rows: Vec<Value> = self.send_json(req).await?;
        let mut out = HashMap::new();
        for row in rows {
            if let (Some(id), Some(count)) = (
                row.get("model_id").and_then(Value::as_str),
                row.get("unread_count").and_then(Value::as_i64),
            ) {
                if count > 0 {
                    out.insert(id.to_owned(), count as u32);
                }
            }
        }
        Ok(out)
    }

    pub async fn clear_unread(&self, model_id: &str) -> Result<bool, DbError> {
        let req = self
            .request(Method::PATCH, "account_notify_state")?
            .header("Prefer", "return=representation")
            .query(&[
                ("model_id", format!("eq.{model_id}")),
                ("unread_count", "gt.0".to_owned()),
                ("select", "model_id".to_owned()),
            ])
            .json(&json!({ "unread_count": 0 }));
        let rows: Vec<Value> = self.send_json(req).await?;
        Ok(!rows.is_empty())
    }

    pub async fn record_employee_sale(
        &self,
        external_key: &str,
        fan_platform_id: &str,
        amount_gross: f64,
        amount_net: f64,
        category: &str,
        recorded_at: &str,
        source_id: &str,
        timezone: &str,
    ) -> Result<(), DbError> {
        if self.active_dropper_id().is_some() {
            return Ok(());
        }
        if external_key.trim().is_empty() || source_id.trim().is_empty() {
            return Ok(());
        }
        let req = self
            .request(Method::POST, "rpc/record_sale")?
            .json(&json!({
                "p_external_key": external_key,
                "p_fan_platform_id": fan_platform_id,
                "p_amount_gross": round2(amount_gross),
                "p_amount_net": round2(amount_net),
                "p_category": category,
                "p_recorded_at": recorded_at,
                "p_source_id": source_id,
                "p_timezone": if timezone.trim().is_empty() {
                    "America/Caracas"
                } else {
                    timezone.trim()
                },
            }));
        match self.send_json::<Value>(req).await {
            Ok(_) => Ok(()),
            Err(err) if sale_record_suppressed(&err) => Ok(()),
            Err(err) => Err(err),
        }
    }
}

fn sale_record_suppressed(err: &DbError) -> bool {
    match err {
        DbError::Auth => true,
        DbError::Http(message) | DbError::Invalid(message) => {
            let lower = message.to_ascii_lowercase();
            lower.contains("turno activo")
                || lower.contains("fuera del horario del turno")
                || lower.contains("no asignado")
                || lower.contains("not allowed")
                || lower.contains("droppers no registran")
        }
        DbError::NotFound(_) => false,
    }
}

fn is_missing_rpc(err: &DbError) -> bool {
    match err {
        DbError::Http(msg) => {
            let lower = msg.to_ascii_lowercase();
            lower.contains("could not find the function")
                || lower.contains("pgrst202")
                || lower.contains("404")
        }
        _ => false,
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn parse_postgrest_error(status: u16, body: &str) -> DbError {
    if status == 401 {
        return DbError::Auth;
    }
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if let Some(message) = value.get("message").and_then(Value::as_str) {
            if !message.trim().is_empty() {
                return DbError::Http(message.trim().to_owned());
            }
        }
    }
    DbError::Http(format!("Supabase respondió {status}"))
}

fn de_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Number(num) => num.as_f64().unwrap_or(0.0),
        Value::String(text) => text.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    })
}

fn de_opt_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Null => None,
        Value::Number(num) => num.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_rfc3339_sql_and_unix_epoch_timestamps() {
        assert_eq!(
            normalize_timestamp_rfc3339("2026-11-26T06:36:52+00:00").as_deref(),
            Some("2026-11-26T06:36:52+00:00")
        );
        assert_eq!(
            normalize_timestamp_rfc3339("2026-11-26 06:36:52").as_deref(),
            Some("2026-11-26T06:36:52+00:00")
        );
        let precise = normalize_timestamp_rfc3339("1795588612.000000").unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(&precise).is_ok());
        assert_eq!(unix_seconds("1795588612.000000"), Some(1795588612));
    }
}
