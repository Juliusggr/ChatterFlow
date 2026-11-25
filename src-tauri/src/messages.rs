use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::db::{utc_now, Database, DbError};
use crate::fans::require_fan_id;

pub const SCRIPT_PAGE_SIZE: i64 = 10;
pub const PPV_PAGE_SIZE: i64 = 10;
/// Observed composer bounds from the packaged messages overlay: `{ min: 3, max: 200 }`.
pub const MEDIA_PRICE_MIN: f64 = 3.0;
pub const MEDIA_PRICE_MAX: f64 = 200.0;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxChat {
    pub of_fan_id: String,
    pub username: String,
    pub nickname: String,
    pub last_body: Option<String>,
    pub last_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptInfo {
    pub script_id: i64,
    pub message_id: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: i64,
    pub body: String,
    pub direction: String,
    pub created_at: String,
    pub media_price: Option<f64>,
    pub locked_text: bool,
    pub media: Vec<Value>,
    pub release_forms: Vec<Value>,
    pub script_infos: Vec<ScriptInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRecord {
    pub id: i64,
    pub script_name: String,
    pub script_content: String,
    pub script_text_content: String,
    pub media_price: Option<f64>,
    pub locked_text: bool,
    pub media: Vec<Value>,
    pub release_forms: Vec<Value>,
    pub tag_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptPage {
    pub script_list: Vec<ScriptRecord>,
    pub tab_list: Vec<String>,
    pub page: i64,
    pub size: i64,
    pub has_next: bool,
    pub items_empty: bool,
    pub media_price_min: f64,
    pub media_price_max: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PpvPoint {
    pub id: i64,
    pub price: Option<f64>,
    pub purchased: bool,
    pub created_at: String,
    pub message_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PpvTimelineState {
    pub ppv_data: Vec<PpvPoint>,
    pub ppv_price: Option<f64>,
    pub ppv_avg_price: Option<f64>,
    pub page: i64,
    pub size: i64,
    pub has_next: bool,
    pub ppv_loading: bool,
    pub first_loading: bool,
    pub items_empty: bool,
    pub pay_count: i64,
    pub all_pay_count: i64,
    pub pay_percent: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundDraft {
    pub body: String,
    pub media_price: Option<f64>,
    pub media: Vec<Value>,
    pub release_forms: Vec<Value>,
    pub locked_text: bool,
    pub script_id: Option<i64>,
}

pub fn list_inbox(database: &Database, model_id: i64) -> Result<Vec<InboxChat>, DbError> {
    database.get_model(model_id)?;
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT ids.of_fan_id,
                    COALESCE(f.username, ''),
                    COALESCE(f.nickname, ''),
                    (
                        SELECT body FROM messages m
                        WHERE m.model_id = ?1 AND m.of_fan_id = ids.of_fan_id
                        ORDER BY m.created_at DESC, m.id DESC
                        LIMIT 1
                    ) AS last_body,
                    (
                        SELECT created_at FROM messages m
                        WHERE m.model_id = ?1 AND m.of_fan_id = ids.of_fan_id
                        ORDER BY m.created_at DESC, m.id DESC
                        LIMIT 1
                    ) AS last_at
             FROM (
                SELECT of_fan_id FROM fans WHERE model_id = ?1
                UNION
                SELECT of_fan_id FROM messages WHERE model_id = ?1
             ) ids
             LEFT JOIN fans f ON f.model_id = ?1 AND f.of_fan_id = ids.of_fan_id
             ORDER BY last_at IS NULL, last_at DESC, ids.of_fan_id",
        )?;
        let rows = stmt.query_map([model_id], |row| {
            Ok(InboxChat {
                of_fan_id: row.get(0)?,
                username: row.get(1)?,
                nickname: row.get(2)?,
                last_body: row.get(3)?,
                last_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}

pub fn list_thread(
    database: &Database,
    model_id: i64,
    of_fan_id: &str,
) -> Result<Vec<ChatMessage>, DbError> {
    let of_fan_id = require_fan_id(of_fan_id)?;
    database.get_model(model_id)?;
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, body, direction, created_at, media_price, locked_text,
                    media_json, release_forms_json, script_infos_json
             FROM messages
             WHERE model_id = ?1 AND of_fan_id = ?2
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![model_id, of_fan_id], read_message)?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}

pub fn list_scripts(
    database: &Database,
    model_id: i64,
    page: i64,
    text: &str,
    tag_id: &str,
) -> Result<ScriptPage, DbError> {
    database.get_model(model_id)?;
    let page = page.max(1);
    let text = text.trim();
    let tag_id = tag_id.trim();
    let like = if text.is_empty() {
        None
    } else {
        Some(format!("%{}%", escape_like(text)))
    };
    database.with_connection(|conn| {
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM scripts
             WHERE model_id = ?1
               AND (?2 = '' OR tag_id = ?2)
               AND (
                    ?3 IS NULL
                    OR script_name LIKE ?3 ESCAPE '\\'
                    OR script_text_content LIKE ?3 ESCAPE '\\'
                    OR script_content LIKE ?3 ESCAPE '\\'
               )",
            params![model_id, tag_id, like],
            |row| row.get(0),
        )?;
        let mut stmt = conn.prepare(
            "SELECT id, script_name, script_content, script_text_content, media_price,
                    locked_text, media_json, release_forms_json, tag_id
             FROM scripts
             WHERE model_id = ?1
               AND (?2 = '' OR tag_id = ?2)
               AND (
                    ?3 IS NULL
                    OR script_name LIKE ?3 ESCAPE '\\'
                    OR script_text_content LIKE ?3 ESCAPE '\\'
                    OR script_content LIKE ?3 ESCAPE '\\'
               )
             ORDER BY id ASC
             LIMIT ?4 OFFSET ?5",
        )?;
        let offset = (page - 1) * SCRIPT_PAGE_SIZE;
        let rows = stmt.query_map(
            params![model_id, tag_id, like, SCRIPT_PAGE_SIZE, offset],
            |row| {
                Ok(ScriptRecord {
                    id: row.get(0)?,
                    script_name: row.get(1)?,
                    script_content: row.get(2)?,
                    script_text_content: row.get(3)?,
                    media_price: row.get(4)?,
                    locked_text: row.get::<_, i64>(5)? != 0,
                    media: parse_json_array(&row.get::<_, String>(6)?),
                    release_forms: parse_json_array(&row.get::<_, String>(7)?),
                    tag_id: row.get(8)?,
                })
            },
        )?;
        let script_list = rows.collect::<Result<Vec<_>, _>>()?;
        let mut tags = conn.prepare(
            "SELECT DISTINCT tag_id FROM scripts
             WHERE model_id = ?1 AND tag_id <> ''
             ORDER BY tag_id COLLATE NOCASE",
        )?;
        let tab_list = tags
            .query_map([model_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ScriptPage {
            script_list,
            tab_list,
            page,
            size: SCRIPT_PAGE_SIZE,
            has_next: page * SCRIPT_PAGE_SIZE < total,
            items_empty: total == 0,
            media_price_min: MEDIA_PRICE_MIN,
            media_price_max: MEDIA_PRICE_MAX,
        })
    })
}

pub fn ppv_timeline(
    database: &Database,
    model_id: i64,
    of_fan_id: &str,
    page: i64,
) -> Result<PpvTimelineState, DbError> {
    let of_fan_id = require_fan_id(of_fan_id)?;
    database.get_model(model_id)?;
    let page = page.max(1);
    database.with_connection(|conn| {
        let (pay_count, all_pay_count, avg_price, latest_price): (i64, i64, Option<f64>, Option<f64>) =
            conn.query_row(
                "SELECT
                    COALESCE(SUM(CASE WHEN purchased = 1 THEN 1 ELSE 0 END), 0),
                    COUNT(*),
                    AVG(price),
                    (
                        SELECT price FROM ppv_timeline
                        WHERE model_id = ?1 AND of_fan_id = ?2
                        ORDER BY created_at DESC, id DESC
                        LIMIT 1
                    )
                 FROM ppv_timeline
                 WHERE model_id = ?1 AND of_fan_id = ?2",
                params![model_id, of_fan_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        let offset = (page - 1) * PPV_PAGE_SIZE;
        let mut stmt = conn.prepare(
            "SELECT id, price, purchased, created_at, message_id
             FROM ppv_timeline
             WHERE model_id = ?1 AND of_fan_id = ?2
             ORDER BY created_at DESC, id DESC
             LIMIT ?3 OFFSET ?4",
        )?;
        let rows = stmt.query_map(params![model_id, of_fan_id, PPV_PAGE_SIZE, offset], |row| {
            Ok(PpvPoint {
                id: row.get(0)?,
                price: row.get(1)?,
                purchased: row.get::<_, i64>(2)? != 0,
                created_at: row.get(3)?,
                message_id: row.get(4)?,
            })
        })?;
        let ppv_data = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(PpvTimelineState {
            ppv_data,
            ppv_price: latest_price,
            ppv_avg_price: avg_price,
            page,
            size: PPV_PAGE_SIZE,
            has_next: page * PPV_PAGE_SIZE < all_pay_count,
            ppv_loading: false,
            first_loading: false,
            items_empty: all_pay_count == 0,
            pay_count,
            all_pay_count,
            pay_percent: if all_pay_count == 0 {
                0.0
            } else {
                pay_count as f64 / all_pay_count as f64
            },
        })
    })
}

pub fn send_message(
    database: &Database,
    model_id: i64,
    of_fan_id: &str,
    draft: OutboundDraft,
) -> Result<ChatMessage, DbError> {
    let of_fan_id = require_fan_id(of_fan_id)?;
    database.get_model(model_id)?;
    let price = normalize_price(draft.media_price)?;
    if !draft.media.is_empty() && !draft.media.iter().all(|item| item.is_object()) {
        return Err(DbError::Invalid(
            "cada media del borrador tiene que ser un objeto".to_owned(),
        ));
    }
    let nickname = database.with_connection(|conn| {
        conn.query_row(
            "SELECT nickname FROM fans WHERE of_fan_id = ?1 AND model_id = ?2",
            params![of_fan_id, model_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
    })?;
    let Some(nickname) = nickname else {
        return Err(DbError::Invalid(
            "ese fan no está en la bandeja de esta modelo".to_owned(),
        ));
    };
    let body = apply_fan_name(draft.body.trim(), nickname.trim());
    if body.is_empty() && draft.media.is_empty() {
        return Err(DbError::Invalid(
            "escribe un mensaje o adjunta media antes de enviar".to_owned(),
        ));
    }
    if body.chars().count() > 10_000 {
        return Err(DbError::Invalid(
            "el mensaje supera los 10000 caracteres".to_owned(),
        ));
    }
    if let Some(script_id) = draft.script_id {
        let owns = database.with_connection(|conn| {
            conn.query_row(
                "SELECT 1 FROM scripts WHERE id = ?1 AND model_id = ?2",
                params![script_id, model_id],
                |_| Ok(true),
            )
            .optional()
        })?;
        if owns.is_none() {
            return Err(DbError::Invalid(
                "el script no pertenece a esta modelo".to_owned(),
            ));
        }
    }
    let media_json = serde_json::to_string(&draft.media).map_err(|err| DbError::Invalid(err.to_string()))?;
    let forms_json =
        serde_json::to_string(&draft.release_forms).map_err(|err| DbError::Invalid(err.to_string()))?;
    let created_at = utc_now();
    database.with_connection(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO messages (
                model_id, of_fan_id, body, direction, created_at, media_price, locked_text,
                media_json, release_forms_json, script_infos_json
             ) VALUES (?1, ?2, ?3, 'out', ?4, ?5, ?6, ?7, ?8, '[]')",
            params![
                model_id,
                of_fan_id,
                body,
                created_at,
                price,
                if draft.locked_text { 1 } else { 0 },
                media_json,
                forms_json
            ],
        )?;
        let message_id = tx.last_insert_rowid();
        let infos = match draft.script_id {
            Some(script_id) => json!([{ "scriptId": script_id, "messageId": message_id }]),
            None => json!([]),
        };
        tx.execute(
            "UPDATE messages SET script_infos_json = ?1 WHERE id = ?2",
            params![infos.to_string(), message_id],
        )?;
        if price.is_some() {
            tx.execute(
                "INSERT INTO ppv_timeline (model_id, of_fan_id, message_id, price, purchased, created_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![model_id, of_fan_id, message_id, price, created_at],
            )?;
        }
        let message = tx.query_row(
            "SELECT id, body, direction, created_at, media_price, locked_text,
                    media_json, release_forms_json, script_infos_json
             FROM messages WHERE id = ?1",
            [message_id],
            read_message,
        )?;
        tx.commit()?;
        Ok(message)
    })
}

pub fn apply_fan_name(text: &str, nickname: &str) -> String {
    if nickname.is_empty() {
        return text.to_owned();
    }
    text.replace("{name}", nickname)
}

fn normalize_price(price: Option<f64>) -> Result<Option<f64>, DbError> {
    let Some(price) = price else {
        return Ok(None);
    };
    if !price.is_finite() || !(MEDIA_PRICE_MIN..=MEDIA_PRICE_MAX).contains(&price) {
        return Err(DbError::Invalid(format!(
            "el precio del mensaje tiene que estar entre {MEDIA_PRICE_MIN} y {MEDIA_PRICE_MAX} USD, o quedar vacío"
        )));
    }
    Ok(Some((price * 100.0).round() / 100.0))
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn parse_json_array(raw: &str) -> Vec<Value> {
    serde_json::from_str::<Vec<Value>>(raw).unwrap_or_default()
}

fn read_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    let infos: Vec<ScriptInfo> =
        serde_json::from_str(&row.get::<_, String>(8)?).unwrap_or_default();
    Ok(ChatMessage {
        id: row.get(0)?,
        body: row.get(1)?,
        direction: row.get(2)?,
        created_at: row.get(3)?,
        media_price: row.get(4)?,
        locked_text: row.get::<_, i64>(5)? != 0,
        media: parse_json_array(&row.get::<_, String>(6)?),
        release_forms: parse_json_array(&row.get::<_, String>(7)?),
        script_infos: infos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_tables_stay_empty_and_pages_concatenate() {
        let dir = std::env::temp_dir().join(format!("creatorfloww-messages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let database = Database::initialize(dir.clone()).await.unwrap();
        let model = database.create_model_partition().unwrap();

        assert!(list_inbox(&database, model.id).unwrap().is_empty());
        let scripts = list_scripts(&database, model.id, 1, "", "").unwrap();
        assert!(scripts.items_empty);
        assert!(scripts.script_list.is_empty());
        assert!(!scripts.has_next);

        database
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO fans (of_fan_id, model_id, username, nickname) VALUES ('fan-1', ?1, 'fan', 'Nico')",
                    [model.id],
                )?;
                for index in 0..12 {
                    conn.execute(
                        "INSERT INTO scripts (model_id, script_name, script_text_content, tag_id)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![
                            model.id,
                            format!("script {index}"),
                            format!("hola {{name}} {index}"),
                            if index < 2 { "vip" } else { "" }
                        ],
                    )?;
                }
                Ok(())
            })
            .unwrap();

        let inbox = list_inbox(&database, model.id).unwrap();
        assert_eq!(inbox.len(), 1);
        assert!(inbox[0].last_body.is_none());

        let first = list_scripts(&database, model.id, 1, "", "").unwrap();
        let second = list_scripts(&database, model.id, 2, "", "").unwrap();
        assert_eq!(first.script_list.len(), 10);
        assert!(first.has_next);
        assert!(!first.items_empty);
        assert_eq!(second.script_list.len(), 2);
        assert!(!second.has_next);
        let tagged = list_scripts(&database, model.id, 1, "script 1", "vip").unwrap();
        assert_eq!(tagged.script_list.len(), 1);

        let sent = send_message(
            &database,
            model.id,
            "fan-1",
            OutboundDraft {
                body: "hola {name}".to_owned(),
                media_price: Some(5.0),
                media: Vec::new(),
                release_forms: Vec::new(),
                locked_text: true,
                script_id: Some(first.script_list[0].id),
            },
        )
        .unwrap();
        assert_eq!(sent.body, "hola Nico");
        assert_eq!(sent.script_infos.len(), 1);
        assert!(sent.locked_text);

        let rate = ppv_timeline(&database, model.id, "fan-1", 1).unwrap();
        assert_eq!(rate.pay_count, 0);
        assert_eq!(rate.all_pay_count, 1);
        assert_eq!(rate.pay_percent, 0.0);
        assert!(!rate.items_empty);

        let empty_rate = ppv_timeline(&database, model.id, "missing", 1).unwrap();
        assert!(empty_rate.items_empty);
        assert_eq!(empty_rate.all_pay_count, 0);

        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }
}
