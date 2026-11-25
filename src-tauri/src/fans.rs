use rusqlite::{params, OptionalExtension};
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

impl Default for FanLists {
    fn default() -> Self {
        Self {
            fans: Vec::new(),
            following: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanSale {
    pub of_transaction_id: String,
    pub kind: String,
    pub gross_amount: f64,
    pub net_amount: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanInsights {
    pub model_id: i64,
    pub of_fan_id: String,
    pub total_spend: f64,
    pub last_spend: Option<String>,
    pub ppv_total: f64,
    pub ppv_count: i64,
    pub tip_total: f64,
    pub tip_count: i64,
    pub highest_spend: Option<f64>,
    pub spending_power: String,
    pub chargeback_risk: String,
    pub sub_status: String,
    pub sub_cost: Option<f64>,
    pub sub_duration: String,
    pub auto_renewal: String,
    pub nickname: String,
    pub birthday: String,
    pub location: String,
    pub country: String,
    pub lists: FanLists,
    pub notes: String,
    pub sales: Vec<FanSale>,
    pub spend_source: String,
}

pub fn fan_insights(
    database: &Database,
    model_id: i64,
    of_fan_id: &str,
) -> Result<FanInsights, DbError> {
    let of_fan_id = of_fan_id.trim();
    if of_fan_id.is_empty() {
        return Ok(empty_insights(model_id, ""));
    }
    database.with_connection(|conn| {
        let profile = conn
            .query_row(
                "SELECT nickname, birthday, location, country, sub_status, sub_cost, sub_duration,
                        auto_renewal, chargeback_risk, lists_json, total_spend, ppv_total, tip_total,
                        official_spend, last_spend_at, highest_spend
                 FROM fans WHERE of_fan_id = ?1",
                [of_fan_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<f64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, f64>(10)?,
                        row.get::<_, f64>(11)?,
                        row.get::<_, f64>(12)?,
                        row.get::<_, i64>(13)?,
                        row.get::<_, Option<String>>(14)?,
                        row.get::<_, f64>(15)?,
                    ))
                },
            )
            .ok();
        let notes = conn
            .query_row(
                "SELECT notes_text FROM fan_notes WHERE of_fan_id = ?1 AND model_id = ?2",
                params![of_fan_id, model_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap_or_default();
        let mut sales_stmt = conn.prepare(
            "SELECT t.of_transaction_id, t.type, t.gross_amount, t.net_amount, t.created_at
             FROM transactions t
             INNER JOIN fans f ON f.id = t.fan_id
             WHERE t.model_id = ?1 AND f.of_fan_id = ?2
             ORDER BY t.created_at DESC, t.id DESC
             LIMIT 50",
        )?;
        let sales = sales_stmt
            .query_map(params![model_id, of_fan_id], |row| {
                Ok(FanSale {
                    of_transaction_id: row.get(0)?,
                    kind: row.get(1)?,
                    gross_amount: row.get(2)?,
                    net_amount: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let (
            nickname,
            birthday,
            location,
            country,
            sub_status,
            sub_cost,
            sub_duration,
            auto_renewal,
            chargeback_risk,
            lists_json,
            official_total,
            stored_ppv,
            stored_tip,
            official_spend,
            stored_last_spend,
            stored_highest,
        ) = profile.unwrap_or_else(|| {
            (
                String::new(),
                None,
                None,
                String::new(),
                None,
                None,
                None,
                None,
                None,
                "{}".to_owned(),
                0.0,
                0.0,
                0.0,
                0,
                None,
                0.0,
            )
        });
        let sales: Vec<FanSale> = sales
            .into_iter()
            .filter(|sale| {
                let id = sale.of_transaction_id.as_str();
                !id.starts_with("chat-") && !id.starts_with("dom-") && !id.starts_with("chart:")
            })
            .map(|mut sale| {
                sale.net_amount = display_net(sale.gross_amount, sale.net_amount);
                sale
            })
            .collect();
        let (timeline_ppv, timeline_count, timeline_last, timeline_high): (f64, i64, Option<String>, Option<f64>) =
            conn.query_row(
                "SELECT COALESCE(SUM(price), 0),
                        COUNT(*),
                        MAX(created_at),
                        MAX(price)
                 FROM ppv_timeline
                 WHERE model_id = ?1 AND of_fan_id = ?2 AND purchased = 1 AND price IS NOT NULL",
                params![model_id, of_fan_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        let (chat_tips, chat_tip_count, chat_tip_last, chat_tip_high): (f64, i64, Option<String>, Option<f64>) =
            conn.query_row(
                "SELECT COALESCE(SUM(amount), 0),
                        COUNT(*),
                        MAX(created_at),
                        MAX(amount)
                 FROM chat_tips
                 WHERE model_id = ?1 AND of_fan_id = ?2",
                params![model_id, of_fan_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        let scanned_ppv: f64 = sales
            .iter()
            .filter(|sale| sale.kind == "message")
            .map(|sale| sale.net_amount)
            .sum();
        let scanned_tip: f64 = sales
            .iter()
            .filter(|sale| sale.kind == "tip")
            .map(|sale| sale.net_amount)
            .sum();
        let scanned: f64 = sales.iter().map(|sale| sale.net_amount).sum();
        let use_official = official_spend == 1;
        let legacy_total = if !use_official && official_total > 0.0 {
            Some((official_total / crate::db::ONLYFANS_NET_RATIO * 100.0).round() / 100.0)
        } else {
            None
        };
        let historical = use_official || legacy_total.is_some();
        let ppv_total = if historical && stored_ppv > 0.0 {
            stored_ppv
        } else if scanned_ppv > 0.0 {
            scanned_ppv
        } else {
            timeline_ppv
        };
        let tip_total = if historical && stored_tip > 0.0 {
            stored_tip
        } else if scanned_tip > 0.0 {
            scanned_tip
        } else {
            chat_tips
        };
        let ppv_count = if historical && stored_ppv > 0.0 {
            0
        } else if scanned_ppv > 0.0 {
            sales.iter().filter(|sale| sale.kind == "message").count() as i64
        } else {
            timeline_count
        };
        let tip_count = if historical && stored_tip > 0.0 {
            0
        } else if scanned_tip > 0.0 {
            sales.iter().filter(|sale| sale.kind == "tip").count() as i64
        } else {
            chat_tip_count
        };
        let count = sales.len() as i64;
        let last_from_sales = sales
            .iter()
            .map(|sale| sale.created_at.as_str())
            .filter(|stamp| !stamp.starts_with("1970-01-01"))
            .max()
            .map(str::to_owned);
        let highest_from_sales = sales
            .iter()
            .filter(|sale| !sale.created_at.starts_with("1970-01-01") || official_total == 0.0)
            .map(|sale| sale.net_amount)
            .fold(None, |best: Option<f64>, amount| Some(best.map_or(amount, |current| current.max(amount))));
        // Prefer live sales, then any persisted CRM-style fields. Official OF totals
        // (totalSumm) do not include last/highest purchase, so keep those when known.
        let last = [last_from_sales, stored_last_spend, timeline_last, chat_tip_last]
            .into_iter()
            .flatten()
            .filter(|stamp| !stamp.is_empty() && !stamp.starts_with("1970-01-01"))
            .max();
        let highest = [highest_from_sales, (stored_highest > 0.0).then_some(stored_highest), timeline_high, chat_tip_high]
            .into_iter()
            .flatten()
            .reduce(f64::max);
        let total_spend = if use_official {
            official_total
        } else if let Some(gross) = legacy_total {
            gross
        } else if scanned > 0.0 {
            scanned + if scanned_tip > 0.0 { 0.0 } else { chat_tips }
        } else {
            timeline_ppv + chat_tips
        };
        let spend_source = if historical { "onlyfans" } else { "transactions" };
        Ok(FanInsights {
            model_id,
            of_fan_id: of_fan_id.to_owned(),
            total_spend,
            last_spend: last,
            ppv_total,
            ppv_count,
            tip_total,
            tip_count,
            highest_spend: highest,
            spending_power: spending_power(count, total_spend),
            chargeback_risk: text_or_unknown(chargeback_risk.as_deref()),
            sub_status: text_or_unknown(sub_status.as_deref()),
            sub_cost,
            sub_duration: text_or_unknown(sub_duration.as_deref()),
            auto_renewal: match auto_renewal {
                Some(1) => "On".to_owned(),
                Some(0) => "Off".to_owned(),
                _ => "Unknown".to_owned(),
            },
            nickname,
            birthday: birthday.unwrap_or_default(),
            location: location.unwrap_or_default(),
            country,
            lists: parse_lists(&lists_json),
            notes,
            sales,
            spend_source: spend_source.to_owned(),
        })
    })
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

pub fn update_fan_profile(
    database: &Database,
    model_id: i64,
    of_fan_id: &str,
    nickname: Option<&str>,
    birthday: Option<&str>,
    location: Option<&str>,
    country: Option<&str>,
) -> Result<(), DbError> {
    let of_fan_id = require_fan_id(of_fan_id)?;
    let nickname = nickname.map(str::trim);
    let location = location.map(str::trim);
    let country = country.map(str::trim);
    let birthday = match birthday {
        Some(value) => Some(normalize_birthday(value)?),
        None => None,
    };
    if nickname.is_some_and(|value| value.chars().count() > 100)
        || location.is_some_and(|value| value.chars().count() > 120)
        || country.is_some_and(|value| value.chars().count() > 120)
    {
        return Err(DbError::Invalid(
            "el apodo, el país o la ubicación son demasiado largos".to_owned(),
        ));
    }
    if nickname.is_none() && birthday.is_none() && location.is_none() && country.is_none() {
        return Ok(());
    }
    database.get_model(model_id)?;
    let owner = database.with_connection(|conn| {
        conn.query_row(
            "SELECT model_id FROM fans WHERE of_fan_id = ?1",
            [of_fan_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
    })?;
    if let Some(owner) = owner {
        if owner != model_id {
            return Err(DbError::Invalid(
                "ese fan no pertenece a la modelo abierta".to_owned(),
            ));
        }
        return database.with_connection(|conn| {
            let mut sets = Vec::new();
            let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(value) = nickname {
                sets.push("nickname = ?");
                values.push(Box::new(value.to_owned()));
            }
            if let Some(value) = &birthday {
                sets.push("birthday = ?");
                values.push(Box::new(value.clone()));
            }
            if let Some(value) = location {
                sets.push("location = ?");
                values.push(Box::new(value.to_owned()));
            }
            if let Some(value) = country {
                sets.push("country = ?");
                values.push(Box::new(value.to_owned()));
            }
            values.push(Box::new(of_fan_id.to_owned()));
            let sql = format!("UPDATE fans SET {} WHERE of_fan_id = ?", sets.join(", "));
            let params = values.iter().map(|value| value.as_ref()).collect::<Vec<_>>();
            conn.execute(&sql, params.as_slice())?;
            Ok(())
        });
    }
    database.with_connection(|conn| {
        conn.execute(
            "INSERT INTO fans (of_fan_id, model_id, username, nickname, birthday, location, country)
             VALUES (?1, ?2, '', ?3, ?4, ?5, ?6)",
            params![
                of_fan_id,
                model_id,
                nickname.unwrap_or(""),
                birthday.flatten(),
                location.unwrap_or(""),
                country.unwrap_or("")
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

pub fn update_fan_lists(
    database: &Database,
    model_id: i64,
    of_fan_id: &str,
    lists: FanLists,
) -> Result<(), DbError> {
    let of_fan_id = require_fan_id(of_fan_id)?;
    let lists = normalize_lists(lists)?;
    let payload = serde_json::to_string(&lists).map_err(|err| DbError::Invalid(err.to_string()))?;
    database.get_model(model_id)?;
    database.with_connection(|conn| {
        conn.execute(
            "INSERT INTO fans (of_fan_id, model_id, username, lists_json)
             VALUES (?1, ?2, '', ?3)
             ON CONFLICT(of_fan_id) DO UPDATE SET
                lists_json = excluded.lists_json",
            params![of_fan_id, model_id, payload],
        )?;
        Ok(())
    })
}

fn empty_insights(model_id: i64, of_fan_id: &str) -> FanInsights {
    FanInsights {
        model_id,
        of_fan_id: of_fan_id.to_owned(),
        total_spend: 0.0,
        last_spend: None,
        ppv_total: 0.0,
        ppv_count: 0,
        tip_total: 0.0,
        tip_count: 0,
        highest_spend: None,
        spending_power: "Insufficient data".to_owned(),
        chargeback_risk: "Unknown".to_owned(),
        sub_status: "Unknown".to_owned(),
        sub_cost: None,
        sub_duration: "Unknown".to_owned(),
        auto_renewal: "Unknown".to_owned(),
        nickname: String::new(),
        birthday: String::new(),
        location: String::new(),
        country: String::new(),
        lists: FanLists::default(),
        spend_source: "transactions".to_owned(),
        notes: String::new(),
        sales: Vec::new(),
    }
}

fn display_net(gross: f64, net: f64) -> f64 {
    if gross > 0.0 && (gross - net).abs() < 0.009 {
        (gross * crate::db::ONLYFANS_NET_RATIO * 100.0).round() / 100.0
    } else if net > 0.0 {
        net
    } else {
        (gross * crate::db::ONLYFANS_NET_RATIO * 100.0).round() / 100.0
    }
}

fn spending_power(spend_count: i64, total: f64) -> String {
    if spend_count < 3 {
        "Insufficient data".to_owned()
    } else if total >= 200.0 {
        "High".to_owned()
    } else if total >= 50.0 {
        "Medium".to_owned()
    } else {
        "Low".to_owned()
    }
}

fn text_or_unknown(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| "Unknown".to_owned())
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

fn normalize_birthday(birthday: &str) -> Result<Option<String>, DbError> {
    let birthday = birthday.trim();
    if birthday.is_empty() {
        return Ok(None);
    }
    let mut parts = birthday.split('-');
    let year = parts.next().and_then(|value| value.parse::<i32>().ok());
    let month = parts.next().and_then(|value| value.parse::<u32>().ok());
    let day = parts.next().and_then(|value| value.parse::<u32>().ok());
    if parts.next().is_some() || year.is_none() || !(1..=12).contains(&month.unwrap_or(0)) || !(1..=31).contains(&day.unwrap_or(0))
    {
        return Err(DbError::Invalid(
            "la fecha de nacimiento debe usar el formato YYYY-MM-DD".to_owned(),
        ));
    }
    Ok(Some(birthday.to_owned()))
}

fn normalize_lists(lists: FanLists) -> Result<FanLists, DbError> {
    Ok(FanLists {
        fans: normalize_items(lists.fans)?,
        following: normalize_items(lists.following)?,
    })
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

fn parse_lists(raw: &str) -> FanLists {
    serde_json::from_str::<FanLists>(raw).unwrap_or_else(|_| FanLists::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[tokio::test]
    async fn empty_fan_stays_empty_and_history_is_exact() {
        let dir = std::env::temp_dir().join(format!(
            "creatorfloww-fans-{}",
            std::process::id()
        ));
        let database = Database::initialize(dir.clone()).await.unwrap();
        let model = database.create_model_partition().unwrap();
        let empty = fan_insights(&database, model.id, "fan-9").unwrap();
        assert_eq!(empty.total_spend, 0.0);
        assert!(empty.last_spend.is_none());
        assert!(empty.highest_spend.is_none());
        assert_eq!(empty.spending_power, "Insufficient data");
        assert_eq!(empty.chargeback_risk, "Unknown");
        assert_eq!(empty.sub_status, "Unknown");

        database
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO fans (of_fan_id, model_id, username) VALUES ('fan-9', ?1, 'fan')",
                    [model.id],
                )?;
                let fan_id: i64 = conn.query_row(
                    "SELECT id FROM fans WHERE of_fan_id = 'fan-9'",
                    [],
                    |row| row.get(0),
                )?;
                for (index, amount) in [10.0_f64, 20.0, 5.0].into_iter().enumerate() {
                    conn.execute(
                        "INSERT INTO transactions (
                            of_transaction_id, model_id, fan_id, type, gross_amount, net_amount, created_at
                         ) VALUES (?1, ?2, ?3, 'message', ?4, ?5, ?6)",
                        params![
                            format!("ppv-{index}"),
                            model.id,
                            fan_id,
                            amount,
                            amount * 0.8,
                            format!("2026-01-0{}T00:00:00Z", index + 1)
                        ],
                    )?;
                }
                Ok(())
            })
            .unwrap();

        let insights = fan_insights(&database, model.id, "fan-9").unwrap();
        assert_eq!(insights.total_spend, 28.0);
        assert_eq!(insights.ppv_total, 28.0);
        assert_eq!(insights.ppv_count, 3);
        assert_eq!(insights.tip_count, 0);
        assert_eq!(insights.highest_spend, Some(16.0));
        assert_eq!(insights.last_spend.as_deref(), Some("2026-01-03T00:00:00Z"));
        assert_eq!(insights.spending_power, "Low");
        assert_eq!(insights.sales.len(), 3);

        save_fan_notes(&database, model.id, "fan-9", "prefiere tips cortos").unwrap();
        update_fan_profile(
            &database,
            model.id,
            "fan-9",
            Some("Nico"),
            Some("1998-04-02"),
            Some("Madrid"),
            None,
        )
        .unwrap();
        update_fan_profile(&database, model.id, "fan-9", None, None, None, Some("España")).unwrap();
        let saved = fan_insights(&database, model.id, "fan-9").unwrap();
        assert_eq!(saved.notes, "prefiere tips cortos");
        assert_eq!(saved.nickname, "Nico");
        assert_eq!(saved.birthday, "1998-04-02");
        assert_eq!(saved.location, "Madrid");
        assert_eq!(saved.country, "España");
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }
}
