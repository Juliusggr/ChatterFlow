use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::{utc_now, Database, DbError};

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

pub fn apply_fan_subscription(
    database: &Database,
    model_id: i64,
    of_fan_id: &str,
    username: &str,
    sub_status: Option<&str>,
    sub_cost: Option<f64>,
    sub_duration: Option<&str>,
    auto_renewal: Option<bool>,
    total_spend: Option<f64>,
    lists: Vec<FanListItem>,
    messages_summ: Option<f64>,
    tips_summ: Option<f64>,
    official_spend: bool,
) -> Result<(), DbError> {
    apply_fan_subscription_with_spend_meta(
        database,
        model_id,
        of_fan_id,
        username,
        sub_status,
        sub_cost,
        sub_duration,
        auto_renewal,
        total_spend,
        lists,
        messages_summ,
        tips_summ,
        official_spend,
        None,
        None,
    )
}

pub fn apply_fan_subscription_with_spend_meta(
    database: &Database,
    model_id: i64,
    of_fan_id: &str,
    username: &str,
    sub_status: Option<&str>,
    sub_cost: Option<f64>,
    sub_duration: Option<&str>,
    auto_renewal: Option<bool>,
    total_spend: Option<f64>,
    lists: Vec<FanListItem>,
    messages_summ: Option<f64>,
    tips_summ: Option<f64>,
    official_spend: bool,
    last_spend_at: Option<&str>,
    highest_spend: Option<f64>,
) -> Result<(), DbError> {
    let of_fan_id = require_fan_id(of_fan_id)?;
    let auto_renewal = auto_renewal.map(|value| if value { 1 } else { 0 });
    let last_spend_at = last_spend_at
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.starts_with("1970-01-01"));
    let lists_json = if lists.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(&FanLists {
                fans: normalize_items(lists)?,
                following: Vec::new(),
            })
            .map_err(|err| DbError::Invalid(err.to_string()))?,
        )
    };
    database.get_model(model_id)?;
    database.with_connection(|conn| {
        conn.execute(
            "INSERT INTO fans (
                of_fan_id, model_id, username, sub_status, sub_cost, sub_duration, auto_renewal,
                total_spend, lists_json, ppv_total, tip_total, official_spend, last_spend_at, highest_spend
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, COALESCE(?8, 0), COALESCE(?9, '[]'),
                       CASE WHEN ?11 IS NOT NULL THEN ?11 ELSE 0 END,
                       CASE WHEN ?12 IS NOT NULL THEN ?12 ELSE 0 END,
                       ?10, ?13, COALESCE(?14, 0))
             ON CONFLICT(of_fan_id) DO UPDATE SET
                username = CASE WHEN excluded.username <> '' THEN excluded.username ELSE fans.username END,
                sub_status = COALESCE(excluded.sub_status, fans.sub_status),
                sub_cost = COALESCE(excluded.sub_cost, fans.sub_cost),
                sub_duration = COALESCE(excluded.sub_duration, fans.sub_duration),
                auto_renewal = COALESCE(excluded.auto_renewal, fans.auto_renewal),
                total_spend = CASE
                    WHEN fans.official_spend = 1 AND ?10 = 0 THEN fans.total_spend
                    ELSE COALESCE(?8, fans.total_spend)
                END,
                ppv_total = CASE WHEN ?11 IS NOT NULL THEN ?11 ELSE fans.ppv_total END,
                tip_total = CASE WHEN ?12 IS NOT NULL THEN ?12 ELSE fans.tip_total END,
                official_spend = CASE WHEN ?10 = 1 THEN 1 ELSE fans.official_spend END,
                lists_json = CASE WHEN ?9 IS NULL THEN fans.lists_json ELSE excluded.lists_json END,
                last_spend_at = CASE
                    WHEN ?13 IS NULL THEN fans.last_spend_at
                    WHEN fans.last_spend_at IS NULL THEN ?13
                    WHEN ?13 > fans.last_spend_at THEN ?13
                    ELSE fans.last_spend_at
                END,
                highest_spend = CASE
                    WHEN ?14 IS NULL THEN fans.highest_spend
                    WHEN ?14 > fans.highest_spend THEN ?14
                    ELSE fans.highest_spend
                END",
            params![
                of_fan_id,
                model_id,
                username.trim(),
                sub_status,
                sub_cost,
                sub_duration,
                auto_renewal,
                total_spend,
                lists_json,
                if official_spend { 1 } else { 0 },
                messages_summ,
                tips_summ,
                last_spend_at,
                highest_spend
            ],
        )?;
        Ok(())
    })
}

pub fn save_fan_notes(
    database: &Database,
    model_id: i64,
    of_fan_id: &str,
    notes_text: &str,
) -> Result<(), DbError> {
    let of_fan_id = require_fan_id(of_fan_id)?;
    if notes_text.chars().count() > 8000 {
        return Err(DbError::Invalid(
            "la nota supera los 8000 caracteres".to_owned(),
        ));
    }
    database.get_model(model_id)?;
    database.with_connection(|conn| {
        conn.execute(
            "INSERT INTO fan_notes (of_fan_id, model_id, notes_text, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(of_fan_id, model_id) DO UPDATE SET
                notes_text = excluded.notes_text,
                updated_at = excluded.updated_at",
            params![of_fan_id, model_id, notes_text, utc_now()],
        )?;
        Ok(())
    })
}

pub(crate) fn require_fan_id(of_fan_id: &str) -> Result<&str, DbError> {
    let of_fan_id = of_fan_id.trim();
    if of_fan_id.is_empty() {
        return Err(DbError::Invalid(
            "no hay un fan activo en el chat".to_owned(),
        ));
    }
    Ok(of_fan_id)
}

fn normalize_items(items: Vec<FanListItem>) -> Result<Vec<FanListItem>, DbError> {
    let mut seen = std::collections::BTreeSet::new();
    let mut normalized = Vec::new();
    for item in items {
        let name = item.name.trim();
        if name.is_empty() {
            continue;
        }
        if name.chars().count() > 60 {
            return Err(DbError::Invalid(
                "el nombre de la lista es demasiado largo".to_owned(),
            ));
        }
        let key = name.to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }
        normalized.push(FanListItem {
            name: name.to_owned(),
            checked: item.checked,
        });
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fan_notes_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "creatorfloww-fans-{}",
            std::process::id()
        ));
        let database = Database::initialize(dir.clone()).await.unwrap();
        let model = database.create_model_partition().unwrap();
        save_fan_notes(&database, model.id, "fan-9", "prefiere tips cortos").unwrap();
        let notes: String = database
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT notes_text FROM fan_notes WHERE of_fan_id = 'fan-9' AND model_id = ?1",
                    [model.id],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(notes, "prefiere tips cortos");
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }
}
