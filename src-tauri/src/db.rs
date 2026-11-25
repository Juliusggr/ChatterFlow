//! Local SQLite database for CreatorFloww.
//!
//! The file `creatorfloww.db` is created in the application data directory.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::TimeZone;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DATABASE_FILE_NAME: &str = "creatorfloww.db";
/// OnlyFans keeps 20%. Net = gross × this ratio.
pub const ONLYFANS_NET_RATIO: f64 = 0.8;
const SCHEMA_VERSION: i32 = 7;
static PARTITION_SEQ: AtomicU64 = AtomicU64::new(0);

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS models (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    of_user_id TEXT UNIQUE,
    username TEXT NOT NULL DEFAULT '',
    display_name TEXT NOT NULL DEFAULT '',
    avatar_url TEXT,
    current_balance REAL NOT NULL DEFAULT 0,
    pending_balance REAL NOT NULL DEFAULT 0,
    last_synced_at TEXT,
    partition_id TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE IF NOT EXISTS employees (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('Owner', 'Admin', 'Chatter', 'Dropper')),
    schedule_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(schedule_json)),
    is_clocked_in INTEGER NOT NULL DEFAULT 0 CHECK (is_clocked_in IN (0, 1)),
    last_clock_in TEXT
) STRICT;

CREATE TABLE IF NOT EXISTS fans (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    of_fan_id TEXT NOT NULL UNIQUE,
    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    username TEXT NOT NULL DEFAULT '',
    nickname TEXT NOT NULL DEFAULT '',
    total_spend REAL NOT NULL DEFAULT 0,
    last_spend_at TEXT,
    highest_spend REAL NOT NULL DEFAULT 0,
    ppv_total REAL NOT NULL DEFAULT 0,
    tip_total REAL NOT NULL DEFAULT 0,
    chargeback_risk TEXT,
    spending_power TEXT,
    sub_status TEXT,
    sub_cost REAL,
    sub_duration TEXT,
    auto_renewal INTEGER CHECK (auto_renewal IS NULL OR auto_renewal IN (0, 1)),
    official_spend INTEGER NOT NULL DEFAULT 0 CHECK (official_spend IN (0, 1)),
    birthday TEXT,
    location TEXT,
    country TEXT NOT NULL DEFAULT '',
    lists_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(lists_json))
) STRICT;

CREATE TABLE IF NOT EXISTS scripts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    script_name TEXT NOT NULL DEFAULT '',
    script_content TEXT NOT NULL DEFAULT '',
    script_text_content TEXT NOT NULL DEFAULT '',
    media_price REAL,
    locked_text INTEGER NOT NULL DEFAULT 0 CHECK (locked_text IN (0, 1)),
    media_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(media_json)),
    release_forms_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(release_forms_json)),
    tag_id TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    of_fan_id TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    direction TEXT NOT NULL CHECK (direction IN ('in', 'out')),
    created_at TEXT NOT NULL,
    media_price REAL,
    locked_text INTEGER NOT NULL DEFAULT 0 CHECK (locked_text IN (0, 1)),
    media_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(media_json)),
    release_forms_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(release_forms_json)),
    script_infos_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(script_infos_json))
) STRICT;

CREATE TABLE IF NOT EXISTS ppv_timeline (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    of_fan_id TEXT NOT NULL,
    message_id INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    price REAL,
    purchased INTEGER NOT NULL DEFAULT 0 CHECK (purchased IN (0, 1)),
    created_at TEXT NOT NULL,
    of_message_id TEXT
) STRICT;

CREATE TABLE IF NOT EXISTS shifts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    employee_id INTEGER NOT NULL REFERENCES employees(id) ON DELETE CASCADE,
    start_time TEXT NOT NULL,
    end_time TEXT,
    total_sales_gross REAL NOT NULL DEFAULT 0,
    total_sales_net REAL NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE IF NOT EXISTS transactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    of_transaction_id TEXT NOT NULL UNIQUE,
    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    fan_id INTEGER REFERENCES fans(id) ON DELETE SET NULL,
    employee_id INTEGER REFERENCES employees(id) ON DELETE SET NULL,
    type TEXT NOT NULL CHECK (type IN ('sub', 'tip', 'post', 'message', 'referral', 'stream')),
    gross_amount REAL NOT NULL,
    net_amount REAL NOT NULL,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS fan_notes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    of_fan_id TEXT NOT NULL,
    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    notes_text TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL,
    UNIQUE (of_fan_id, model_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_fans_model_id ON fans (model_id);
CREATE INDEX IF NOT EXISTS idx_shifts_employee_id ON shifts (employee_id);
CREATE INDEX IF NOT EXISTS idx_transactions_model_id_created_at ON transactions (model_id, created_at);
CREATE INDEX IF NOT EXISTS idx_transactions_fan_id ON transactions (fan_id);
CREATE INDEX IF NOT EXISTS idx_transactions_employee_id ON transactions (employee_id);
CREATE INDEX IF NOT EXISTS idx_fan_notes_model_id ON fan_notes (model_id);
CREATE INDEX IF NOT EXISTS idx_scripts_model_id ON scripts (model_id, tag_id);
CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages (model_id, of_fan_id, created_at);
CREATE INDEX IF NOT EXISTS idx_ppv_timeline_fan ON ppv_timeline (model_id, of_fan_id, created_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_ppv_timeline_of_message
    ON ppv_timeline (model_id, of_message_id)
    WHERE of_message_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS chat_tips (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    of_fan_id TEXT NOT NULL,
    of_message_id TEXT NOT NULL,
    amount REAL NOT NULL,
    created_at TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_tips_message ON chat_tips (model_id, of_message_id);
CREATE INDEX IF NOT EXISTS idx_chat_tips_fan ON chat_tips (model_id, of_fan_id, created_at);

CREATE TABLE IF NOT EXISTS account_notify_state (
    model_id INTEGER PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE,
    last_seen_message_id TEXT NOT NULL DEFAULT '',
    last_message_timestamp TEXT,
    unread_count INTEGER NOT NULL DEFAULT 0,
    inbox_seeded INTEGER NOT NULL DEFAULT 0 CHECK (inbox_seeded IN (0, 1)),
    transactions_seeded INTEGER NOT NULL DEFAULT 0 CHECK (transactions_seeded IN (0, 1))
) STRICT;

CREATE TABLE IF NOT EXISTS notified_events (
    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    event_key TEXT NOT NULL,
    notified_at TEXT NOT NULL,
    PRIMARY KEY (model_id, event_key)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_notified_events_at ON notified_events (notified_at);

CREATE TABLE IF NOT EXISTS notification_history (
    id TEXT PRIMARY KEY,
    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    model_name TEXT NOT NULL DEFAULT '',
    model_username TEXT NOT NULL DEFAULT '',
    model_avatar TEXT,
    of_fan_id TEXT NOT NULL DEFAULT '',
    fan_name TEXT NOT NULL DEFAULT '',
    amount REAL,
    net_amount REAL,
    preview TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_notification_history_created ON notification_history (created_at);
";

#[derive(Debug)]
pub enum DbError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    Task(String),
    Schema(String),
    Invalid(String),
    Poisoned,
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Io(err) => write!(f, "no se pudo preparar el directorio de la base: {err}"),
            DbError::Sqlite(err) => write!(f, "error de SQLite: {err}"),
            DbError::Task(err) => write!(f, "la inicialización de la base fue cancelada: {err}"),
            DbError::Schema(message) | DbError::Invalid(message) => write!(f, "{message}"),
            DbError::Poisoned => write!(f, "la conexión SQLite quedó inutilizable"),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DbError::Io(err) => Some(err),
            DbError::Sqlite(err) => Some(err),
            DbError::Task(_) | DbError::Schema(_) | DbError::Invalid(_) | DbError::Poisoned => None,
        }
    }
}

impl From<std::io::Error> for DbError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<rusqlite::Error> for DbError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmployeeRole {
    Owner,
    Admin,
    Chatter,
    Dropper,
}

impl EmployeeRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::Admin => "Admin",
            Self::Chatter => "Chatter",
            Self::Dropper => "Dropper",
        }
    }

    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "Owner" => Ok(Self::Owner),
            "Admin" => Ok(Self::Admin),
            "Chatter" => Ok(Self::Chatter),
            "Dropper" => Ok(Self::Dropper),
            other => Err(DbError::Invalid(format!("rol no válido: {other}"))),
        }
    }
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
    pub id: i64,
    pub of_user_id: Option<String>,
    pub username: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub current_balance: f64,
    pub pending_balance: f64,
    pub last_synced_at: Option<String>,
    pub partition_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Employee {
    pub id: i64,
    pub name: String,
    pub role: EmployeeRole,
    pub schedule_json: Value,
    pub is_clocked_in: bool,
    pub last_clock_in: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shift {
    pub id: i64,
    pub employee_id: i64,
    pub start_time: String,
    pub end_time: Option<String>,
    pub total_sales_gross: f64,
    pub total_sales_net: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fan {
    pub id: i64,
    pub of_fan_id: String,
    pub model_id: i64,
    pub username: String,
    pub nickname: String,
    pub total_spend: f64,
    pub last_spend_at: Option<String>,
    pub highest_spend: f64,
    pub ppv_total: f64,
    pub tip_total: f64,
    pub chargeback_risk: Option<String>,
    pub spending_power: Option<String>,
    pub sub_status: Option<String>,
    pub sub_cost: Option<f64>,
    pub sub_duration: Option<String>,
    pub auto_renewal: Option<bool>,
    pub birthday: Option<String>,
    pub location: Option<String>,
    pub lists_json: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: i64,
    pub of_transaction_id: String,
    pub model_id: i64,
    pub fan_id: i64,
    pub employee_id: Option<i64>,
    #[serde(rename = "type")]
    pub kind: TransactionType,
    pub gross_amount: f64,
    pub net_amount: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanNote {
    pub id: i64,
    pub of_fan_id: String,
    pub model_id: i64,
    pub notes_text: String,
    pub updated_at: String,
}

pub fn utc_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[derive(Debug)]
pub struct Database {
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl Database {
    pub async fn initialize(app_data_dir: impl Into<PathBuf>) -> Result<Self, DbError> {
        let app_data_dir = app_data_dir.into();
        match tokio::task::spawn_blocking(move || initialize_blocking(app_data_dir)).await {
            Ok(result) => result,
            Err(err) => Err(DbError::Task(err.to_string())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn with_connection<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    {
        let conn = self.lock()?;
        f(&conn).map_err(DbError::Sqlite)
    }

    pub fn with_transaction<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, rusqlite::Error>,
    {
        let mut conn = self.lock()?;
        let tx = conn.transaction().map_err(DbError::Sqlite)?;
        let value = f(&tx).map_err(DbError::Sqlite)?;
        tx.commit().map_err(DbError::Sqlite)?;
        Ok(value)
    }

    pub fn list_employees(&self) -> Result<Vec<Employee>, DbError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, role, schedule_json, is_clocked_in, last_clock_in
             FROM employees
             ORDER BY name COLLATE NOCASE, id",
        )?;
        let rows = stmt.query_map([], read_employee)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    pub fn add_employee(
        &self,
        name: &str,
        role: EmployeeRole,
        schedule_json: &Value,
    ) -> Result<Employee, DbError> {
        let name = normalize_name(name)?;
        let schedule = normalize_schedule(schedule_json)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO employees (name, role, schedule_json, is_clocked_in)
             VALUES (?1, ?2, ?3, 0)",
            params![name, role.as_str(), schedule],
        )?;
        let employee = read_employee_by_id(&tx, tx.last_insert_rowid())?;
        tx.commit()?;
        Ok(employee)
    }

    pub fn update_employee_role_schedule(
        &self,
        id: i64,
        role: EmployeeRole,
        schedule_json: &Value,
    ) -> Result<Employee, DbError> {
        let schedule = normalize_schedule(schedule_json)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE employees SET role = ?2, schedule_json = ?3 WHERE id = ?1",
            params![id, role.as_str(), schedule],
        )?;
        if changed == 0 {
            return Err(DbError::Invalid(format!("no existe el empleado {id}")));
        }
        let employee = read_employee_by_id(&tx, id)?;
        tx.commit()?;
        Ok(employee)
    }

    pub fn delete_employee(&self, id: i64) -> Result<(), DbError> {
        let conn = self.lock()?;
        let changed = conn.execute("DELETE FROM employees WHERE id = ?1", [id])?;
        if changed == 0 {
            return Err(DbError::Invalid(format!("no existe el empleado {id}")));
        }
        Ok(())
    }

    pub fn toggle_clock_in(&self, id: i64) -> Result<Employee, DbError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let clocked: Option<i64> = tx
            .query_row(
                "SELECT is_clocked_in FROM employees WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(clocked) = clocked else {
            return Err(DbError::Invalid(format!("no existe el empleado {id}")));
        };
        let now = utc_now();
        if clocked == 0 {
            tx.execute(
                "UPDATE employees SET is_clocked_in = 1, last_clock_in = ?2 WHERE id = ?1",
                params![id, &now],
            )?;
            let open: i64 = tx.query_row(
                "SELECT COUNT(*) FROM shifts WHERE employee_id = ?1 AND end_time IS NULL",
                [id],
                |row| row.get(0),
            )?;
            if open == 0 {
                tx.execute(
                    "INSERT INTO shifts (employee_id, start_time) VALUES (?1, ?2)",
                    params![id, &now],
                )?;
            }
        } else {
            tx.execute(
                "UPDATE employees SET is_clocked_in = 0 WHERE id = ?1",
                [id],
            )?;
            tx.execute(
                "UPDATE shifts SET end_time = ?2 WHERE employee_id = ?1 AND end_time IS NULL",
                params![id, &now],
            )?;
        }
        let employee = read_employee_by_id(&tx, id)?;
        tx.commit()?;
        Ok(employee)
    }

    pub fn list_models(&self) -> Result<Vec<Model>, DbError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, of_user_id, username, display_name, avatar_url, current_balance,
                    pending_balance, last_synced_at, partition_id
             FROM models
             ORDER BY display_name COLLATE NOCASE, username COLLATE NOCASE, id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Model {
                id: row.get(0)?,
                of_user_id: row.get(1)?,
                username: row.get(2)?,
                display_name: row.get(3)?,
                avatar_url: row.get(4)?,
                current_balance: row.get(5)?,
                pending_balance: row.get(6)?,
                last_synced_at: row.get(7)?,
                partition_id: row.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    pub fn get_model(&self, id: i64) -> Result<Model, DbError> {
        let conn = self.lock()?;
        read_model_by_id(&conn, id)
    }

    pub fn create_model_partition(&self) -> Result<Model, DbError> {
        let partition_id = next_partition_id();
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO models (username, display_name, partition_id) VALUES ('', '', ?1)",
            [&partition_id],
        )?;
        read_model_by_id(&conn, conn.last_insert_rowid())
    }

    /// Finds an existing unauthenticated placeholder model whose window is not currently open,
    /// avoiding duplicate "Unknown" rows when a user re-clicks "Agregar modelo".
    pub fn find_reusable_empty_model(&self, open_model_ids: &[i64]) -> Result<Option<Model>, DbError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, of_user_id, username, display_name, avatar_url, current_balance,
                    pending_balance, last_synced_at, partition_id
             FROM models
             WHERE of_user_id IS NULL AND username = '' AND display_name = ''
             ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], read_model)?;
        for row in rows {
            let model = row?;
            if !open_model_ids.contains(&model.id) {
                return Ok(Some(model));
            }
        }
        Ok(None)
    }

    /// When a newly created placeholder partition logs into an OnlyFans account that already
    /// exists in `models`, adopt the fresh `partition_id` onto the existing model and remove
    /// the empty placeholder row so no historical metrics are lost and no ghost row remains.
    pub fn adopt_partition_into_existing_model(
        &self,
        placeholder_id: i64,
        of_user_id: &str,
        username: &str,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<Option<(Model, String)>, DbError> {
        let of_user_id = of_user_id.trim();
        if of_user_id.is_empty() {
            return Ok(None);
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let existing_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM models WHERE of_user_id = ?1 AND id != ?2",
                params![of_user_id, placeholder_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(existing_id) = existing_id else {
            return Ok(None);
        };
        let placeholder = read_model_by_id(&tx, placeholder_id)?;
        if placeholder.of_user_id.is_some() {
            return Ok(None);
        }
        let existing = read_model_by_id(&tx, existing_id)?;
        let old_partition = existing.partition_id;
        let fresh_partition = placeholder.partition_id;
        let synced_at = utc_now();

        tx.execute("DELETE FROM models WHERE id = ?1", [placeholder_id])?;
        tx.execute(
            "UPDATE models
             SET partition_id = ?2,
                 username = CASE WHEN ?3 <> '' THEN ?3 ELSE username END,
                 display_name = CASE WHEN ?4 <> '' THEN ?4 ELSE display_name END,
                 avatar_url = COALESCE(?5, avatar_url),
                 last_synced_at = ?6
             WHERE id = ?1",
            params![
                existing_id,
                fresh_partition,
                username.trim(),
                display_name.trim(),
                avatar_url,
                synced_at
            ],
        )?;
        let updated = read_model_by_id(&tx, existing_id)?;
        tx.commit()?;
        Ok(Some((updated, old_partition)))
    }

    pub fn delete_model(&self, id: i64) -> Result<(), DbError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let changed = tx.execute("DELETE FROM models WHERE id = ?1", [id])?;
        if changed == 0 {
            return Err(DbError::Invalid(format!("no existe la modelo {id}")));
        }
        recompute_shift_totals(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn save_onlyfans_profile(
        &self,
        model_id: i64,
        of_user_id: &str,
        username: &str,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<Model, DbError> {
        let of_user_id = of_user_id.trim();
        if of_user_id.is_empty() {
            return Err(DbError::Invalid(
                "OnlyFans no devolvió el id de la cuenta".to_owned(),
            ));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let owner: Option<i64> = tx
            .query_row(
                "SELECT id FROM models WHERE of_user_id = ?1 AND id != ?2",
                params![of_user_id, model_id],
                |row| row.get(0),
            )
            .optional()?;
        if owner.is_some() {
            return Err(DbError::Invalid(
                "esa cuenta de OnlyFans ya está agregada".to_owned(),
            ));
        }
        let exists: Option<i64> = tx
            .query_row("SELECT id FROM models WHERE id = ?1", [model_id], |row| row.get(0))
            .optional()?;
        if exists.is_none() {
            return Err(DbError::Invalid(format!("no existe la modelo {model_id}")));
        }
        let synced_at = utc_now();
        tx.execute(
            "UPDATE models
             SET of_user_id = ?2,
                 username = CASE WHEN ?3 <> '' THEN ?3 ELSE username END,
                 display_name = CASE WHEN ?4 <> '' THEN ?4 ELSE display_name END,
                 avatar_url = COALESCE(?5, avatar_url),
                 last_synced_at = ?6
             WHERE id = ?1",
            params![
                model_id,
                of_user_id,
                username.trim(),
                display_name.trim(),
                avatar_url,
                synced_at
            ],
        )?;
        let model = read_model_by_id(&tx, model_id)?;
        tx.commit()?;
        Ok(model)
    }

    pub fn save_onlyfans_balances(
        &self,
        model_id: i64,
        current: Option<f64>,
        pending: Option<f64>,
    ) -> Result<(), DbError> {
        if current.is_none() && pending.is_none() {
            return Ok(());
        }
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE models
             SET current_balance = COALESCE(?2, current_balance),
                 pending_balance = COALESCE(?3, pending_balance),
                 last_synced_at = ?4
             WHERE id = ?1",
            params![model_id, current, pending, utc_now()],
        )?;
        if changed == 0 {
            return Err(DbError::Invalid(format!("no existe la modelo {model_id}")));
        }
        Ok(())
    }

    pub fn delete_chart_span(
        &self,
        model_id: i64,
        channel: &str,
        start_unix: i64,
        end_unix: i64,
    ) -> Result<(), DbError> {
        let pattern = format!("chart:{model_id}:{channel}:%");
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM transactions
             WHERE model_id = ?1
               AND of_transaction_id LIKE ?2
               AND creatorfloww_unix(created_at) >= ?3
               AND creatorfloww_unix(created_at) < ?4",
            params![model_id, pattern, start_unix, end_unix],
        )?;
        Ok(())
    }

    pub fn upsert_earnings(
        &self,
        model_id: i64,
        earnings: &[EarningDraft],
    ) -> Result<EarningStats, DbError> {
        self.upsert_earnings_with_new(model_id, earnings)
            .map(|(stats, _)| stats)
    }

    pub fn upsert_earnings_with_new(
        &self,
        model_id: i64,
        earnings: &[EarningDraft],
    ) -> Result<(EarningStats, Vec<EarningDraft>), DbError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let exists: Option<i64> = tx
            .query_row("SELECT id FROM models WHERE id = ?1", [model_id], |row| row.get(0))
            .optional()?;
        if exists.is_none() {
            return Err(DbError::Invalid(format!("no existe la modelo {model_id}")));
        }
        let mut upserted = 0_u64;
        let mut linked = 0_u64;
        let mut newly_inserted = Vec::new();
        for earning in earnings {
            let fan_id = match earning.fan_of_id.as_deref() {
                Some(of_fan_id) if !of_fan_id.is_empty() => {
                    Some(upsert_fan(&tx, model_id, of_fan_id, &earning.fan_username)?)
                }
                _ => None,
            };
            let previous_employee: Option<Option<i64>> = tx
                .query_row(
                    "SELECT employee_id FROM transactions WHERE of_transaction_id = ?1",
                    [&earning.of_transaction_id],
                    |row| row.get(0),
                )
                .optional()?;
            let is_new = previous_employee.is_none();
            tx.execute(
                "INSERT INTO transactions (
                    of_transaction_id, model_id, fan_id, employee_id, type,
                    gross_amount, net_amount, created_at
                 ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7)
                 ON CONFLICT(of_transaction_id) DO UPDATE SET
                    model_id = excluded.model_id,
                    fan_id = COALESCE(excluded.fan_id, transactions.fan_id),
                    type = excluded.type,
                    gross_amount = excluded.gross_amount,
                    net_amount = excluded.net_amount,
                    created_at = excluded.created_at",
                params![
                    earning.of_transaction_id,
                    model_id,
                    fan_id,
                    earning.kind.as_str(),
                    earning.gross_amount,
                    earning.net_amount,
                    earning.created_at
                ],
            )?;
            upserted += 1;
            if is_new {
                newly_inserted.push(earning.clone());
            }
            if is_new && matches!(earning.kind, TransactionType::Message | TransactionType::Tip) {
                if let Some(employee_id) = active_shift_employee(&tx, &earning.created_at)? {
                    let changed = tx.execute(
                        "UPDATE transactions
                         SET employee_id = ?2
                         WHERE of_transaction_id = ?1 AND employee_id IS NULL",
                        params![earning.of_transaction_id, employee_id],
                    )?;
                    if changed > 0 {
                        linked += 1;
                    }
                }
            }
        }
        if upserted > 0 {
            tx.execute(
                "UPDATE models SET last_synced_at = ?2 WHERE id = ?1",
                params![model_id, utc_now()],
            )?;
        }
        recompute_shift_totals(&tx)?;
        tx.commit()?;
        Ok((EarningStats { upserted, linked }, newly_inserted))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, DbError> {
        self.conn.lock().map_err(|_| DbError::Poisoned)
    }
}

fn read_employee(row: &rusqlite::Row<'_>) -> rusqlite::Result<Employee> {
    let role: String = row.get(2)?;
    let schedule: String = row.get(3)?;
    let clocked: i64 = row.get(4)?;
    let role = EmployeeRole::parse(&role).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(err),
        )
    })?;
    let schedule_json = serde_json::from_str(&schedule).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(err),
        )
    })?;
    Ok(Employee {
        id: row.get(0)?,
        name: row.get(1)?,
        role,
        schedule_json,
        is_clocked_in: clocked != 0,
        last_clock_in: row.get(5)?,
    })
}

fn read_model(row: &rusqlite::Row<'_>) -> rusqlite::Result<Model> {
    Ok(Model {
        id: row.get(0)?,
        of_user_id: row.get(1)?,
        username: row.get(2)?,
        display_name: row.get(3)?,
        avatar_url: row.get(4)?,
        current_balance: row.get(5)?,
        pending_balance: row.get(6)?,
        last_synced_at: row.get(7)?,
        partition_id: row.get(8)?,
    })
}

fn read_model_by_id(conn: &Connection, id: i64) -> Result<Model, DbError> {
    match conn.query_row(
        "SELECT id, of_user_id, username, display_name, avatar_url, current_balance,
                pending_balance, last_synced_at, partition_id
         FROM models WHERE id = ?1",
        [id],
        read_model,
    ) {
        Ok(model) => Ok(model),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            Err(DbError::Invalid(format!("no existe la modelo {id}")))
        }
        Err(err) => Err(err.into()),
    }
}

fn next_partition_id() -> String {
    let seq = PARTITION_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("part-{}-{seq}-{nanos}", std::process::id())
}

fn parse_timestamp(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

fn upsert_fan(
    conn: &Connection,
    model_id: i64,
    of_fan_id: &str,
    username: &str,
) -> Result<i64, DbError> {
    conn.execute(
        "INSERT INTO fans (of_fan_id, model_id, username) VALUES (?1, ?2, ?3)
         ON CONFLICT(of_fan_id) DO UPDATE SET
            username = CASE WHEN excluded.username <> '' THEN excluded.username ELSE fans.username END",
        params![of_fan_id, model_id, username.trim()],
    )?;
    conn.query_row(
        "SELECT id FROM fans WHERE of_fan_id = ?1",
        [of_fan_id],
        |row| row.get(0),
    )
    .map_err(DbError::from)
}

fn active_shift_employee(conn: &Connection, created_at: &str) -> Result<Option<i64>, DbError> {
    let Some(created) = parse_timestamp(created_at) else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT e.id, s.start_time
         FROM shifts s
         JOIN employees e ON e.id = s.employee_id
         WHERE e.is_clocked_in = 1 AND s.end_time IS NULL
         ORDER BY s.start_time DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (employee_id, start_raw) = row?;
        if let Some(start) = parse_timestamp(&start_raw) {
            if created >= start {
                return Ok(Some(employee_id));
            }
        }
    }
    Ok(None)
}

fn recompute_shift_totals(conn: &Connection) -> Result<(), DbError> {
    let mut shift_stmt =
        conn.prepare("SELECT id, employee_id, start_time, end_time FROM shifts")?;
    let shifts: Vec<(i64, i64, String, Option<String>)> = shift_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(shift_stmt);

    let mut sale_stmt = conn.prepare(
        "SELECT employee_id, gross_amount, net_amount, created_at
         FROM transactions
         WHERE employee_id IS NOT NULL AND type IN ('message', 'tip')",
    )?;
    let sales: Vec<(i64, f64, f64, String)> = sale_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(sale_stmt);

    for (shift_id, employee_id, start_raw, end_raw) in shifts {
        let Some(start) = parse_timestamp(&start_raw) else {
            continue;
        };
        let end = end_raw.as_deref().and_then(parse_timestamp);
        let mut gross = 0.0;
        let mut net = 0.0;
        for (sale_employee, sale_gross, sale_net, created_raw) in &sales {
            if *sale_employee != employee_id {
                continue;
            }
            let Some(created) = parse_timestamp(created_raw) else {
                continue;
            };
            if created < start {
                continue;
            }
            if let Some(end) = end {
                if created > end {
                    continue;
                }
            }
            gross += sale_gross;
            net += sale_net;
        }
        conn.execute(
            "UPDATE shifts SET total_sales_gross = ?2, total_sales_net = ?3 WHERE id = ?1",
            params![shift_id, gross, net],
        )?;
    }
    Ok(())
}

fn read_employee_by_id(conn: &Connection, id: i64) -> Result<Employee, DbError> {
    conn.query_row(
        "SELECT id, name, role, schedule_json, is_clocked_in, last_clock_in
         FROM employees WHERE id = ?1",
        [id],
        read_employee,
    )
    .map_err(DbError::from)
}

fn normalize_name(name: &str) -> Result<String, DbError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(DbError::Invalid(
            "el nombre del empleado es obligatorio".to_owned(),
        ));
    }
    if trimmed.chars().count() > 120 {
        return Err(DbError::Invalid(
            "el nombre supera 120 caracteres".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn normalize_schedule(value: &Value) -> Result<String, DbError> {
    let Some(object) = value.as_object() else {
        return Err(DbError::Invalid(
            "el horario debe ser un objeto JSON".to_owned(),
        ));
    };
    if let Some(days) = object.get("days") {
        let Some(days) = days.as_array() else {
            return Err(DbError::Invalid("days debe ser una lista".to_owned()));
        };
        for day in days {
            let Some(day) = day.as_object() else {
                return Err(DbError::Invalid(
                    "cada día del horario debe ser un objeto".to_owned(),
                ));
            };
            let name = day.get("day").and_then(Value::as_str).unwrap_or("");
            if !matches!(name, "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun") {
                return Err(DbError::Invalid(format!("día de horario no válido: {name}")));
            }
            let Some(enabled) = day.get("enabled").and_then(Value::as_bool) else {
                return Err(DbError::Invalid(
                    "enabled debe ser verdadero o falso".to_owned(),
                ));
            };
            let start = day.get("start").and_then(Value::as_str).unwrap_or("");
            let end = day.get("end").and_then(Value::as_str).unwrap_or("");
            if enabled {
                validate_clock_time(start)?;
                validate_clock_time(end)?;
                if start >= end {
                    return Err(DbError::Invalid(
                        "la hora de inicio debe ser anterior a la de fin".to_owned(),
                    ));
                }
            }
        }
    }
    serde_json::to_string(value).map_err(|err| DbError::Invalid(err.to_string()))
}

fn validate_clock_time(value: &str) -> Result<(), DbError> {
    let Some((hour, minute)) = value.split_once(':') else {
        return Err(DbError::Invalid("usa el formato HH:MM".to_owned()));
    };
    let Ok(hour) = hour.parse::<u32>() else {
        return Err(DbError::Invalid("usa el formato HH:MM".to_owned()));
    };
    let Ok(minute) = minute.parse::<u32>() else {
        return Err(DbError::Invalid("usa el formato HH:MM".to_owned()));
    };
    if value.len() != 5 || hour > 23 || minute > 59 {
        return Err(DbError::Invalid("usa el formato HH:MM".to_owned()));
    }
    Ok(())
}

fn initialize_blocking(app_data_dir: PathBuf) -> Result<Database, DbError> {
    std::fs::create_dir_all(&app_data_dir)?;
    let path = app_data_dir.join(DATABASE_FILE_NAME);
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_FULL_MUTEX;
    let conn = Connection::open_with_flags(&path, flags)?;
    configure(&conn)?;
    migrate(&conn)?;
    install_dashboard_time(&conn)?;
    install_earnings_view(&conn)?;
    Ok(Database {
        path,
        conn: Mutex::new(conn),
    })
}

fn install_dashboard_time(conn: &Connection) -> Result<(), DbError> {
    let flags = rusqlite::functions::FunctionFlags::SQLITE_UTF8
        | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("creatorfloww_unix", 1, flags, |ctx| {
        let value = ctx.get::<String>(0).ok();
        Ok(value.as_deref().and_then(unix_seconds))
    })?;
    conn.create_scalar_function("creatorfloww_local_day", 1, flags, |ctx| {
        let value = ctx.get::<String>(0).ok();
        Ok(value.as_deref().and_then(local_day_key))
    })?;
    Ok(())
}

fn unix_seconds(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|parsed| parsed.timestamp())
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|naive| naive.and_utc().timestamp())
        })
}

fn local_day_key(text: &str) -> Option<String> {
    let zoned = chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|parsed| parsed.with_timezone(&chrono::Local))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|naive| naive.and_utc().with_timezone(&chrono::Local))
        })?;
    Some(zoned.date_naive().format("%Y-%m-%d").to_string())
}

/// Unix seconds of local midnight on `date`.
pub fn local_midnight(date: chrono::NaiveDate) -> i64 {
    let naive = date.and_time(chrono::NaiveTime::from_hms_opt(0, 0, 0).expect("midnight"));
    match chrono::Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(value) => value.timestamp(),
        chrono::LocalResult::Ambiguous(earliest, _) => earliest.timestamp(),
        chrono::LocalResult::None => naive.and_utc().timestamp(),
    }
}

/// First instant that is no longer "today" in the local timezone.
pub fn next_local_midnight() -> i64 {
    local_midnight(chrono::Local::now().date_naive() + chrono::Duration::days(1))
}

fn install_earnings_view(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        "DROP VIEW IF EXISTS model_earnings;
         CREATE VIEW model_earnings AS
         SELECT *
         FROM transactions AS t
         WHERE t.created_at != '1970-01-01T00:00:00Z'
           AND (
             t.of_transaction_id LIKE 'chart:' || t.model_id || ':%'
             OR NOT EXISTS (
               SELECT 1
               FROM transactions AS c
               WHERE c.model_id = t.model_id
                 AND c.of_transaction_id LIKE 'chart:' || t.model_id || ':%'
                 AND c.type = t.type
                 AND creatorfloww_local_day(c.created_at) = creatorfloww_local_day(t.created_at)
                 AND creatorfloww_local_day(t.created_at) IS NOT NULL
             )
           )
           AND NOT (
             t.of_transaction_id LIKE 'dom-%'
             AND EXISTS (
               SELECT 1
               FROM transactions AS chat
               WHERE chat.model_id = t.model_id
                 AND chat.fan_id = t.fan_id
                 AND chat.of_transaction_id LIKE 'chat-%'
                 AND ABS(chat.gross_amount - t.gross_amount) < 0.01
             )
           );",
    )?;
    Ok(())
}

fn configure(conn: &Connection) -> Result<(), DbError> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    let journal: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if !journal.eq_ignore_ascii_case("wal") {
        return Err(DbError::Schema(format!(
            "SQLite no activó el modo WAL (recibido: {journal})"
        )));
    }
    let foreign_keys: i64 = conn.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    if foreign_keys != 1 {
        return Err(DbError::Schema(
            "SQLite no activó las claves foráneas".to_owned(),
        ));
    }
    Ok(())
}

fn migrate(conn: &Connection) -> Result<(), DbError> {
    let current: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(DbError::Schema(format!(
            "creatorfloww.db está en la versión {current}; este binario soporta hasta la versión {SCHEMA_VERSION}"
        )));
    }
    if current == 0 {
        apply_sql(conn, SCHEMA_SQL)?;
        return Ok(());
    }
    if current < 2 {
        upgrade_v1_to_v2(conn)?;
    }
    if current < 3 {
        upgrade_v2_to_v3(conn)?;
    }
    if current < 4 {
        upgrade_v3_to_v4(conn)?;
    }
    if current < 5 {
        upgrade_v4_to_v5(conn)?;
    }
    if current < 6 {
        upgrade_v5_to_v6(conn)?;
    }
    if current < 7 {
        upgrade_v6_to_v7(conn)?;
    }
    Ok(())
}

fn upgrade_v6_to_v7(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let upgraded = (|| -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS account_notify_state (
                model_id INTEGER PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE,
                last_seen_message_id TEXT NOT NULL DEFAULT '',
                last_message_timestamp TEXT,
                unread_count INTEGER NOT NULL DEFAULT 0,
                inbox_seeded INTEGER NOT NULL DEFAULT 0 CHECK (inbox_seeded IN (0, 1)),
                transactions_seeded INTEGER NOT NULL DEFAULT 0 CHECK (transactions_seeded IN (0, 1))
            ) STRICT;
            CREATE TABLE IF NOT EXISTS notified_events (
                model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
                event_key TEXT NOT NULL,
                notified_at TEXT NOT NULL,
                PRIMARY KEY (model_id, event_key)
            ) STRICT;
            CREATE INDEX IF NOT EXISTS idx_notified_events_at ON notified_events (notified_at);
            CREATE TABLE IF NOT EXISTS notification_history (
                id TEXT PRIMARY KEY,
                model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                model_name TEXT NOT NULL DEFAULT '',
                model_username TEXT NOT NULL DEFAULT '',
                model_avatar TEXT,
                of_fan_id TEXT NOT NULL DEFAULT '',
                fan_name TEXT NOT NULL DEFAULT '',
                amount REAL,
                net_amount REAL,
                preview TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL
            ) STRICT;
            CREATE INDEX IF NOT EXISTS idx_notification_history_created ON notification_history (created_at);
            PRAGMA user_version = 7;
            ",
        )?;
        conn.execute_batch("COMMIT")?;
        Ok(())
    })();
    if let Err(err) = upgraded {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(DbError::Sqlite(err));
    }
    Ok(())
}

fn upgrade_v5_to_v6(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let upgraded = (|| -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS chat_tips (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
                of_fan_id TEXT NOT NULL,
                of_message_id TEXT NOT NULL,
                amount REAL NOT NULL,
                created_at TEXT NOT NULL
            ) STRICT;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_tips_message ON chat_tips (model_id, of_message_id);
            CREATE INDEX IF NOT EXISTS idx_chat_tips_fan ON chat_tips (model_id, of_fan_id, created_at);
            PRAGMA user_version = 6;
            ",
        )?;
        conn.execute_batch("COMMIT")?;
        Ok(())
    })();
    if let Err(err) = upgraded {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(DbError::Sqlite(err));
    }
    Ok(())
}

fn upgrade_v4_to_v5(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let upgraded = (|| -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "
            ALTER TABLE ppv_timeline ADD COLUMN of_message_id TEXT;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_ppv_timeline_of_message
                ON ppv_timeline (model_id, of_message_id)
                WHERE of_message_id IS NOT NULL;
            PRAGMA user_version = 5;
            ",
        )?;
        conn.execute_batch("COMMIT")?;
        Ok(())
    })();
    if let Err(err) = upgraded {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(DbError::Sqlite(err));
    }
    Ok(())
}

fn apply_sql(conn: &Connection, sql: &str) -> Result<(), DbError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let migrated = (|| -> Result<(), rusqlite::Error> {
        conn.execute_batch(sql)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        conn.execute_batch("COMMIT")?;
        Ok(())
    })();
    if let Err(err) = migrated {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(DbError::Sqlite(err));
    }
    Ok(())
}

fn upgrade_v1_to_v2(conn: &Connection) -> Result<(), DbError> {
    conn.pragma_update(None, "foreign_keys", 0)?;
    let upgraded = (|| -> Result<(), rusqlite::Error> {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        conn.execute_batch(
            "
            CREATE TABLE models_next (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                of_user_id TEXT UNIQUE,
                username TEXT NOT NULL DEFAULT '',
                display_name TEXT NOT NULL DEFAULT '',
                avatar_url TEXT,
                current_balance REAL NOT NULL DEFAULT 0,
                pending_balance REAL NOT NULL DEFAULT 0,
                last_synced_at TEXT,
                partition_id TEXT NOT NULL UNIQUE
            ) STRICT;
            INSERT INTO models_next (
                id, of_user_id, username, display_name, avatar_url,
                current_balance, pending_balance, last_synced_at, partition_id
            )
            SELECT id, of_user_id, username, display_name, avatar_url,
                   current_balance, pending_balance, last_synced_at, partition_id
            FROM models;
            DROP TABLE models;
            ALTER TABLE models_next RENAME TO models;

            CREATE TABLE transactions_next (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                of_transaction_id TEXT NOT NULL UNIQUE,
                model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
                fan_id INTEGER REFERENCES fans(id) ON DELETE SET NULL,
                employee_id INTEGER REFERENCES employees(id) ON DELETE SET NULL,
                type TEXT NOT NULL CHECK (type IN ('sub', 'tip', 'post', 'message', 'referral', 'stream')),
                gross_amount REAL NOT NULL,
                net_amount REAL NOT NULL,
                created_at TEXT NOT NULL
            ) STRICT;
            INSERT INTO transactions_next (
                id, of_transaction_id, model_id, fan_id, employee_id, type,
                gross_amount, net_amount, created_at
            )
            SELECT id, of_transaction_id, model_id, fan_id, employee_id, type,
                   gross_amount, net_amount, created_at
            FROM transactions;
            DROP TABLE transactions;
            ALTER TABLE transactions_next RENAME TO transactions;
            CREATE INDEX IF NOT EXISTS idx_transactions_model_id_created_at ON transactions (model_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_transactions_fan_id ON transactions (fan_id);
            CREATE INDEX IF NOT EXISTS idx_transactions_employee_id ON transactions (employee_id);
            ",
        )?;
        conn.execute_batch("PRAGMA user_version = 2")?;
        conn.execute_batch("COMMIT")?;
        Ok(())
    })();
    let _ = conn.pragma_update(None, "foreign_keys", 1);
    if let Err(err) = upgraded {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(DbError::Sqlite(err));
    }
    Ok(())
}

fn upgrade_v3_to_v4(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let upgraded = (|| -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "
            ALTER TABLE fans ADD COLUMN official_spend INTEGER NOT NULL DEFAULT 0 CHECK (official_spend IN (0, 1));
            PRAGMA user_version = 4;
            ",
        )?;
        conn.execute_batch("COMMIT")?;
        Ok(())
    })();
    if let Err(err) = upgraded {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(DbError::Sqlite(err));
    }
    Ok(())
}

fn upgrade_v2_to_v3(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let upgraded = (|| -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "
            ALTER TABLE fans ADD COLUMN country TEXT NOT NULL DEFAULT '';
            CREATE TABLE IF NOT EXISTS scripts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
                script_name TEXT NOT NULL DEFAULT '',
                script_content TEXT NOT NULL DEFAULT '',
                script_text_content TEXT NOT NULL DEFAULT '',
                media_price REAL,
                locked_text INTEGER NOT NULL DEFAULT 0 CHECK (locked_text IN (0, 1)),
                media_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(media_json)),
                release_forms_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(release_forms_json)),
                tag_id TEXT NOT NULL DEFAULT ''
            ) STRICT;
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
                of_fan_id TEXT NOT NULL,
                body TEXT NOT NULL DEFAULT '',
                direction TEXT NOT NULL CHECK (direction IN ('in', 'out')),
                created_at TEXT NOT NULL,
                media_price REAL,
                locked_text INTEGER NOT NULL DEFAULT 0 CHECK (locked_text IN (0, 1)),
                media_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(media_json)),
                release_forms_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(release_forms_json)),
                script_infos_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(script_infos_json))
            ) STRICT;
            CREATE TABLE IF NOT EXISTS ppv_timeline (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
                of_fan_id TEXT NOT NULL,
                message_id INTEGER REFERENCES messages(id) ON DELETE SET NULL,
                price REAL,
                purchased INTEGER NOT NULL DEFAULT 0 CHECK (purchased IN (0, 1)),
                created_at TEXT NOT NULL
            ) STRICT;
            CREATE INDEX IF NOT EXISTS idx_scripts_model_id ON scripts (model_id, tag_id);
            CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages (model_id, of_fan_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_ppv_timeline_fan ON ppv_timeline (model_id, of_fan_id, created_at);
            PRAGMA user_version = 3;
            ",
        )?;
        conn.execute_batch("COMMIT")?;
        Ok(())
    })();
    if let Err(err) = upgraded {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(DbError::Sqlite(err));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempAppDir(PathBuf);

    impl TempAppDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("reloj del sistema")
                .as_nanos();
            let seq = TEMP_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "creatorfloww-db-{}-{}-{}",
                std::process::id(),
                seq,
                nanos
            ));
            Self(path)
        }
    }

    impl Drop for TempAppDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn assert_constraint(result: rusqlite::Result<usize>, needle: &str) {
        let message = result.expect_err("la restricción debía rechazar la fila").to_string();
        assert!(
            message.contains(needle),
            "se esperaba '{needle}' en '{message}'"
        );
    }

    #[tokio::test]
    async fn schema_enforces_uniqueness_and_reopens() {
        let dir = TempAppDir::new();
        let db = Database::initialize(dir.0.clone()).await.expect("abrir base");
        assert_eq!(db.path().file_name().and_then(|name| name.to_str()), Some(DATABASE_FILE_NAME));

        db.with_connection(|conn| {
            let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
            assert_eq!(version, SCHEMA_VERSION);

            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )?;
            let tables: Vec<String> = stmt
                .query_map([], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(
                tables,
                vec![
                    "account_notify_state",
                    "chat_tips",
                    "employees",
                    "fan_notes",
                    "fans",
                    "messages",
                    "models",
                    "notification_history",
                    "notified_events",
                    "ppv_timeline",
                    "scripts",
                    "shifts",
                    "transactions",
                ]
            );

            conn.execute(
                "INSERT INTO models (of_user_id, username, display_name, partition_id)
                 VALUES ('of-user-1', 'model_one', 'Model One', 'partition-1')",
                [],
            )?;
            let model_id = conn.last_insert_rowid();
            assert_constraint(
                conn.execute(
                    "INSERT INTO models (of_user_id, username, display_name, partition_id)
                     VALUES ('of-user-1', 'other', 'Other', 'partition-2')",
                    [],
                ),
                "UNIQUE constraint failed: models.of_user_id",
            );

            conn.execute(
                "INSERT INTO models (of_user_id, username, display_name, partition_id)
                 VALUES ('of-user-2', 'model_two', 'Model Two', 'partition-2')",
                [],
            )?;
            let second_model_id = conn.last_insert_rowid();

            conn.execute(
                "INSERT INTO employees (name, role, schedule_json, is_clocked_in)
                 VALUES (?1, ?2, ?3, 1)",
                params!["Ana", EmployeeRole::Chatter.as_str(), r#"{"days":["mon"]}"#],
            )?;
            let employee_id = conn.last_insert_rowid();
            assert_constraint(
                conn.execute(
                    "INSERT INTO employees (name, role) VALUES ('Bad', 'Manager')",
                    [],
                ),
                "CHECK constraint failed",
            );
            assert_constraint(
                conn.execute(
                    "INSERT INTO employees (name, role, schedule_json) VALUES ('Bad', 'Admin', 'no-json')",
                    [],
                ),
                "CHECK constraint failed",
            );

            conn.execute(
                "INSERT INTO fans (of_fan_id, model_id, username, lists_json)
                 VALUES ('fan-1', ?1, 'fan', '[]')",
                [model_id],
            )?;
            let fan_id = conn.last_insert_rowid();
            assert_constraint(
                conn.execute(
                    "INSERT INTO fans (of_fan_id, model_id) VALUES ('fan-1', ?1)",
                    [second_model_id],
                ),
                "UNIQUE constraint failed: fans.of_fan_id",
            );

            conn.execute(
                "INSERT INTO shifts (employee_id, start_time) VALUES (?1, ?2)",
                params![employee_id, utc_now()],
            )?;

            conn.execute(
                "INSERT INTO transactions (
                    of_transaction_id, model_id, fan_id, employee_id, type,
                    gross_amount, net_amount, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    "tx-1",
                    model_id,
                    fan_id,
                    Some(employee_id),
                    TransactionType::Tip.as_str(),
                    12.5_f64,
                    10.0_f64,
                    utc_now(),
                ],
            )?;
            assert_constraint(
                conn.execute(
                    "INSERT INTO transactions (
                        of_transaction_id, model_id, fan_id, type, gross_amount, net_amount, created_at
                     ) VALUES ('tx-1', ?1, ?2, 'tip', 1, 0.8, ?3)",
                    params![model_id, fan_id, utc_now()],
                ),
                "UNIQUE constraint failed: transactions.of_transaction_id",
            );
            conn.execute(
                "INSERT INTO transactions (
                    of_transaction_id, model_id, fan_id, employee_id, type,
                    gross_amount, net_amount, created_at
                 ) VALUES ('tx-2', ?1, ?2, NULL, 'sub', 5, 4, ?3)",
                params![model_id, fan_id, utc_now()],
            )?;
            assert_constraint(
                conn.execute(
                    "INSERT INTO transactions (
                        of_transaction_id, model_id, fan_id, type, gross_amount, net_amount, created_at
                     ) VALUES ('tx-3', ?1, ?2, 'bonus', 1, 1, ?3)",
                    params![model_id, fan_id, utc_now()],
                ),
                "CHECK constraint failed",
            );
            assert_constraint(
                conn.execute(
                    "INSERT INTO transactions (
                        of_transaction_id, model_id, fan_id, type, gross_amount, net_amount, created_at
                     ) VALUES ('tx-4', 9999, ?1, 'tip', 1, 0.8, ?2)",
                    params![fan_id, utc_now()],
                ),
                "FOREIGN KEY constraint failed",
            );

            conn.execute(
                "INSERT INTO fan_notes (of_fan_id, model_id, notes_text, updated_at)
                 VALUES ('fan-1', ?1, 'primera nota', ?2)",
                params![model_id, utc_now()],
            )?;
            assert_constraint(
                conn.execute(
                    "INSERT INTO fan_notes (of_fan_id, model_id, notes_text, updated_at)
                     VALUES ('fan-1', ?1, 'duplicada', ?2)",
                    params![model_id, utc_now()],
                ),
                "UNIQUE constraint failed: fan_notes.of_fan_id, fan_notes.model_id",
            );
            conn.execute(
                "INSERT INTO fan_notes (of_fan_id, model_id, notes_text, updated_at)
                 VALUES ('fan-1', ?1, 'otra modelo', ?2)",
                params![second_model_id, utc_now()],
            )?;

            let employee_json = serde_json::to_string(&Employee {
                id: employee_id,
                name: "Ana".to_owned(),
                role: EmployeeRole::Chatter,
                schedule_json: serde_json::json!({"days": ["mon"]}),
                is_clocked_in: true,
                last_clock_in: None,
            })
            .expect("serializar empleado");
            assert!(employee_json.contains("\"role\":\"Chatter\""));
            let transaction_json = serde_json::to_string(&Transaction {
                id: 1,
                of_transaction_id: "tx-1".to_owned(),
                model_id,
                fan_id,
                employee_id: None,
                kind: TransactionType::Message,
                gross_amount: 1.0,
                net_amount: 0.8,
                created_at: utc_now(),
            })
            .expect("serializar transacción");
            assert!(transaction_json.contains("\"type\":\"message\""));
            Ok(())
        })
        .expect("comprobar esquema");

        drop(db);
        let reopened = Database::initialize(dir.0.clone()).await.expect("reabrir base");
        reopened
            .with_connection(|conn| {
                let count: i64 = conn.query_row("SELECT COUNT(*) FROM transactions", [], |row| row.get(0))?;
                assert_eq!(count, 2);
                Ok(())
            })
            .expect("los datos persisten al reabrir");
    }

    #[tokio::test]
    async fn employee_shift_cycle_rejects_bad_input_and_persists() {
        let dir = TempAppDir::new();
        let db = Database::initialize(dir.0.clone()).await.expect("abrir base");
        let models = db.list_models().expect("modelos");
        assert!(models.is_empty());

        let schedule = serde_json::json!({
            "days": [{ "day": "mon", "enabled": true, "start": "09:00", "end": "17:00" }]
        });
        assert!(db
            .add_employee("   ", EmployeeRole::Chatter, &schedule)
            .is_err());
        assert!(db
            .add_employee(
                "Ana",
                EmployeeRole::Chatter,
                &serde_json::json!({"days":[{"day":"mon","enabled":true,"start":"","end":""}]})
            )
            .is_err());

        let created = db
            .add_employee("  Ana  ", EmployeeRole::Chatter, &schedule)
            .expect("alta");
        assert_eq!(created.name, "Ana");
        assert!(!created.is_clocked_in);

        let clocked_in = db.toggle_clock_in(created.id).expect("clock in");
        assert!(clocked_in.is_clocked_in);
        assert!(clocked_in.last_clock_in.is_some());
        db.with_connection(|conn| {
            let open: i64 = conn.query_row(
                "SELECT COUNT(*) FROM shifts WHERE employee_id = ?1 AND end_time IS NULL",
                [created.id],
                |row| row.get(0),
            )?;
            assert_eq!(open, 1);
            Ok(())
        })
        .expect("turno abierto");

        let clocked_out = db.toggle_clock_in(created.id).expect("clock out");
        assert!(!clocked_out.is_clocked_in);
        assert_eq!(clocked_out.last_clock_in, clocked_in.last_clock_in);

        let again = db.toggle_clock_in(created.id).expect("segundo clock in");
        assert!(again.is_clocked_in);
        db.with_connection(|conn| {
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM shifts WHERE employee_id = ?1",
                [created.id],
                |row| row.get(0),
            )?;
            let open: i64 = conn.query_row(
                "SELECT COUNT(*) FROM shifts WHERE employee_id = ?1 AND end_time IS NULL",
                [created.id],
                |row| row.get(0),
            )?;
            assert_eq!(total, 2);
            assert_eq!(open, 1);
            Ok(())
        })
        .expect("un solo turno abierto");

        let updated = db
            .update_employee_role_schedule(
                created.id,
                EmployeeRole::Admin,
                &serde_json::json!({
                    "days": [{ "day": "tue", "enabled": true, "start": "10:00", "end": "14:00" }]
                }),
            )
            .expect("actualizar");
        assert_eq!(updated.role, EmployeeRole::Admin);
        assert!(updated.is_clocked_in);

        db.delete_employee(created.id).expect("borrar");
        assert!(db.list_employees().expect("lista").is_empty());
        db.with_connection(|conn| {
            let shifts: i64 = conn.query_row("SELECT COUNT(*) FROM shifts", [], |row| row.get(0))?;
            assert_eq!(shifts, 0);
            Ok(())
        })
        .expect("los turnos se van con el empleado");

        drop(db);
        let reopened = Database::initialize(dir.0.clone()).await.expect("reabrir");
        assert!(reopened.list_employees().expect("vacio").is_empty());
    }

    #[tokio::test]
    async fn version_one_database_keeps_rows_and_accepts_a_null_fan() {
        let dir = TempAppDir::new();
        std::fs::create_dir_all(&dir.0).unwrap();
        {
            let conn = Connection::open(dir.0.join(DATABASE_FILE_NAME)).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE models (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    of_user_id TEXT NOT NULL UNIQUE,
                    username TEXT NOT NULL,
                    display_name TEXT NOT NULL DEFAULT '',
                    avatar_url TEXT,
                    current_balance REAL NOT NULL DEFAULT 0,
                    pending_balance REAL NOT NULL DEFAULT 0,
                    last_synced_at TEXT,
                    partition_id TEXT NOT NULL
                ) STRICT;
                CREATE TABLE fans (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    of_fan_id TEXT NOT NULL UNIQUE,
                    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
                    username TEXT NOT NULL DEFAULT '',
                    nickname TEXT NOT NULL DEFAULT '',
                    total_spend REAL NOT NULL DEFAULT 0,
                    last_spend_at TEXT,
                    highest_spend REAL NOT NULL DEFAULT 0,
                    ppv_total REAL NOT NULL DEFAULT 0,
                    tip_total REAL NOT NULL DEFAULT 0,
                    chargeback_risk TEXT,
                    spending_power TEXT,
                    sub_status TEXT,
                    sub_cost REAL,
                    sub_duration TEXT,
                    auto_renewal INTEGER,
                    birthday TEXT,
                    location TEXT,
                    lists_json TEXT NOT NULL DEFAULT '[]'
                ) STRICT;
                CREATE TABLE employees (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    role TEXT NOT NULL,
                    schedule_json TEXT NOT NULL DEFAULT '{}',
                    is_clocked_in INTEGER NOT NULL DEFAULT 0,
                    last_clock_in TEXT
                ) STRICT;
                CREATE TABLE shifts (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    employee_id INTEGER NOT NULL,
                    start_time TEXT NOT NULL,
                    end_time TEXT,
                    total_sales_gross REAL NOT NULL DEFAULT 0,
                    total_sales_net REAL NOT NULL DEFAULT 0
                ) STRICT;
                CREATE TABLE transactions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    of_transaction_id TEXT NOT NULL UNIQUE,
                    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
                    fan_id INTEGER NOT NULL REFERENCES fans(id) ON DELETE CASCADE,
                    employee_id INTEGER REFERENCES employees(id) ON DELETE SET NULL,
                    type TEXT NOT NULL,
                    gross_amount REAL NOT NULL,
                    net_amount REAL NOT NULL,
                    created_at TEXT NOT NULL
                ) STRICT;
                INSERT INTO models (of_user_id, username, display_name, partition_id)
                VALUES ('55', 'kept', 'Kept', 'part-old');
                INSERT INTO fans (of_fan_id, model_id, username) VALUES ('fan-old', 1, 'fan');
                INSERT INTO transactions (
                    of_transaction_id, model_id, fan_id, type, gross_amount, net_amount, created_at
                ) VALUES ('old-tx', 1, 1, 'tip', 3, 2.4, '2026-01-01T00:00:00Z');
                PRAGMA user_version = 1;
                ",
            )
            .unwrap();
        }

        let database = Database::initialize(dir.0.clone()).await.unwrap();
        let model = database.get_model(1).unwrap();
        assert_eq!(model.of_user_id.as_deref(), Some("55"));
        database
            .upsert_earnings(
                model.id,
                &[EarningDraft {
                    of_transaction_id: "no-fan".to_owned(),
                    fan_of_id: None,
                    fan_username: String::new(),
                    kind: TransactionType::Tip,
                    gross_amount: 4.0,
                    net_amount: 3.2,
                    created_at: "2026-01-02T00:00:00Z".to_owned(),
                }],
            )
            .unwrap();
        database
            .with_connection(|conn| {
                let count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM transactions", [], |row| row.get(0))?;
                let version: i32 =
                    conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
                let country: String = conn.query_row(
                    "SELECT country FROM fans WHERE of_fan_id = 'fan-old'",
                    [],
                    |row| row.get(0),
                )?;
                let scripts: i64 =
                    conn.query_row("SELECT COUNT(*) FROM scripts", [], |row| row.get(0))?;
                assert_eq!(count, 2);
                let official: i64 = conn.query_row(
                    "SELECT official_spend FROM fans WHERE of_fan_id = 'fan-old'",
                    [],
                    |row| row.get(0),
                )?;
                let ppv_ids: i64 = conn.query_row(
                    "SELECT COUNT(of_message_id) FROM ppv_timeline",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(version, SCHEMA_VERSION);
                assert_eq!(ppv_ids, 0);
                assert_eq!(official, 0);
                assert_eq!(country, "");
                assert_eq!(scripts, 0);
                Ok(())
            })
            .unwrap();
    }
}
