use std::collections::BTreeMap;

use chrono::{Datelike, Duration, Local};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::{Database, DbError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyticsRange {
    Hours24,
    Days7,
    Days14,
    Days30,
    Days90,
    Days180,
    Year(i32),
    Month(i32, u32),
    All,
}

impl Serialize for AnalyticsRange {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            AnalyticsRange::Hours24 => serializer.serialize_str("24h"),
            AnalyticsRange::Days7 => serializer.serialize_str("7"),
            AnalyticsRange::Days14 => serializer.serialize_str("14"),
            AnalyticsRange::Days30 => serializer.serialize_str("30"),
            AnalyticsRange::Days90 => serializer.serialize_str("90"),
            AnalyticsRange::Days180 => serializer.serialize_str("180"),
            AnalyticsRange::Year(year) => serializer.serialize_str(&format!("year:{year}")),
            AnalyticsRange::Month(year, month) => {
                serializer.serialize_str(&format!("month:{year:04}-{month:02}"))
            }
            AnalyticsRange::All => serializer.serialize_str("all"),
        }
    }
}

impl<'de> Deserialize<'de> for AnalyticsRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RangeVisitor;

        impl<'de> serde::de::Visitor<'de> for RangeVisitor {
            type Value = AnalyticsRange;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "a valid analytics range like '24h', '7', '30', '90', '180', 'year:2026', 'month:2026-11', or 'all'",
                )
            }

            fn visit_str<E>(self, value: &str) -> Result<AnalyticsRange, E>
            where
                E: serde::de::Error,
            {
                let clean = value.trim().to_lowercase();
                if clean == "24h" || clean == "24" || clean == "1" || clean == "day" || clean == "today" {
                    return Ok(AnalyticsRange::Hours24);
                }
                if clean == "7" || clean == "7d" {
                    return Ok(AnalyticsRange::Days7);
                }
                if clean == "14" || clean == "14d" {
                    return Ok(AnalyticsRange::Days14);
                }
                if clean == "30" || clean == "30d" {
                    return Ok(AnalyticsRange::Days30);
                }
                if clean == "90" || clean == "90d" {
                    return Ok(AnalyticsRange::Days90);
                }
                if clean == "180" || clean == "180d" {
                    return Ok(AnalyticsRange::Days180);
                }
                if clean == "all"
                    || clean == "historic"
                    || clean == "historico"
                    || clean == "all_time"
                    || clean == "todo"
                {
                    return Ok(AnalyticsRange::All);
                }
                if let Some(rest) = clean.strip_prefix("year:") {
                    if let Ok(year) = rest.parse::<i32>() {
                        return Ok(AnalyticsRange::Year(year));
                    }
                }
                if let Some(rest) = clean.strip_prefix("month:") {
                    if let Some((y_str, m_str)) = rest.split_once('-') {
                        if let (Ok(y), Ok(m)) = (y_str.parse::<i32>(), m_str.parse::<u32>()) {
                            if (1..=12).contains(&m) {
                                return Ok(AnalyticsRange::Month(y, m));
                            }
                        }
                    }
                }
                if clean.len() == 4 && clean.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(year) = clean.parse::<i32>() {
                        return Ok(AnalyticsRange::Year(year));
                    }
                }
                if clean.len() == 7 && clean.chars().nth(4) == Some('-') {
                    if let Some((y_str, m_str)) = clean.split_once('-') {
                        if let (Ok(y), Ok(m)) = (y_str.parse::<i32>(), m_str.parse::<u32>()) {
                            if (1..=12).contains(&m) {
                                return Ok(AnalyticsRange::Month(y, m));
                            }
                        }
                    }
                }
                Ok(AnalyticsRange::Days7)
            }
        }

        deserializer.deserialize_any(RangeVisitor)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TrendBucket {
    Hour,
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricCard {
    pub key: String,
    pub label: String,
    pub amount: f64,
    pub change_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatorCard {
    pub id: i64,
    pub name: String,
    pub username: String,
    pub avatar_url: Option<String>,
    pub current_balance: f64,
    pub pending_balance: f64,
    pub last_synced_at: Option<String>,
    pub total: f64,
    pub channels: Vec<ChannelAmount>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelAmount {
    pub key: String,
    pub label: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendPoint {
    pub label: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelPoint {
    pub label: String,
    pub subs: f64,
    pub tips: f64,
    pub posts: f64,
    pub ppv: f64,
    pub referrals: f64,
    pub streams: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownSlice {
    pub key: String,
    pub label: String,
    pub amount: f64,
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatorSeries {
    pub id: i64,
    pub name: String,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatorDonut {
    pub id: i64,
    pub name: String,
    pub slices: Vec<BreakdownSlice>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsReport {
    pub cards: Vec<MetricCard>,
    pub creators: Vec<CreatorCard>,
    pub trends: Vec<TrendPoint>,
    pub channels_over_time: Vec<ChannelPoint>,
    pub breakdown: Vec<BreakdownSlice>,
    pub comparison_labels: Vec<String>,
    pub comparison_series: Vec<CreatorSeries>,
    pub comparison_donuts: Vec<CreatorDonut>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub saved: bool,
    pub path: Option<String>,
}

const CHANNELS: &[(&str, &str, &str)] = &[
    ("sub", "subscriptions", "Suscripciones"),
    ("tip", "tips", "Propinas (Tips)"),
    ("post", "posts", "Publicaciones"),
    ("message", "messages", "Mensajes (PPV)"),
    ("referral", "referrals", "Referencias"),
    ("stream", "streams", "Transmisiones"),
];

pub fn report(
    database: &Database,
    range: AnalyticsRange,
    model_ids: &[i64],
    compare_ids: &[i64],
    net: bool,
    bucket: TrendBucket,
) -> Result<AnalyticsReport, DbError> {
    let column = if net { "net_amount" } else { "gross_amount" };
    let (start, end, previous) = bounds(range);
    let current = channel_sums(database, start, end, model_ids, column)?;
    let cards = metric_cards(&current, previous, database, model_ids, column)?;
    let creators = creator_cards(database, start, end, model_ids, column)?;
    let trends = trend(database, start, end, model_ids, column, bucket)?;
    let channels_over_time = channel_series(database, start, end, model_ids, column)?;
    let breakdown = breakdown_slices(&current);
    let (comparison_labels, comparison_series) =
        compare_daily(database, start, end, compare_ids, column)?;
    let compared = if compare_ids.is_empty() {
        Vec::new()
    } else {
        creator_cards(database, start, end, compare_ids, column)?
    };
    let comparison_donuts = compare_ids
        .iter()
        .filter_map(|id| compared.iter().find(|creator| creator.id == *id))
        .map(|creator| CreatorDonut {
            id: creator.id,
            name: creator.name.clone(),
            slices: breakdown_slices(
                &creator
                    .channels
                    .iter()
                    .map(|channel| {
                        (
                            CHANNELS
                                .iter()
                                .find(|item| item.1 == channel.key)
                                .map(|item| item.0.to_owned())
                                .unwrap_or_else(|| channel.key.clone()),
                            channel.amount,
                        )
                    })
                    .collect(),
            ),
        })
        .collect();
    Ok(AnalyticsReport {
        cards,
        creators,
        trends,
        channels_over_time,
        breakdown,
        comparison_labels,
        comparison_series,
        comparison_donuts,
    })
}

pub fn export_report(
    database: &Database,
    range: AnalyticsRange,
    model_ids: &[i64],
    format: &str,
) -> Result<ExportResult, DbError> {
    let rows = export_rows(database, range, model_ids)?;
    let dialog = rfd::FileDialog::new().set_title("Exportar reporte");
    let picked = match format {
        "csv" => dialog
            .set_file_name("creatorfloww.csv")
            .add_filter("CSV", &["csv"])
            .save_file(),
        "xlsx" => dialog
            .set_file_name("creatorfloww.xlsx")
            .add_filter("Excel", &["xlsx"])
            .save_file(),
        "pdf" => dialog
            .set_file_name("creatorfloww.pdf")
            .add_filter("PDF", &["pdf"])
            .save_file(),
        _ => return Err(DbError::Invalid("formato de exportación no válido".to_owned())),
    };
    let Some(path) = picked else {
        return Ok(ExportResult {
            saved: false,
            path: None,
        });
    };
    match format {
        "csv" => write_csv(&path, &rows)?,
        "xlsx" => write_xlsx(&path, &rows)?,
        "pdf" => write_pdf(&path, &rows)?,
        _ => unreachable!(),
    }
    Ok(ExportResult {
        saved: true,
        path: Some(path.display().to_string()),
    })
}

struct ExportRow {
    created_at: String,
    creator: String,
    kind: String,
    gross: f64,
    net: f64,
    of_transaction_id: String,
}

fn export_rows(
    database: &Database,
    range: AnalyticsRange,
    model_ids: &[i64],
) -> Result<Vec<ExportRow>, DbError> {
    let (start, end, _) = bounds(range);
    let filter = model_filter("t.model_id", model_ids);
    let sql = format!(
        "SELECT t.created_at, COALESCE(NULLIF(m.display_name, ''), m.username), t.type,
                t.gross_amount, t.net_amount, t.of_transaction_id
         FROM model_earnings t
         JOIN models m ON m.id = t.model_id
         WHERE (?1 IS NULL OR creatorfloww_unix(t.created_at) >= ?1)
           AND (?2 IS NULL OR creatorfloww_unix(t.created_at) < ?2)
           {filter}
         ORDER BY t.created_at"
    );
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![start, end], |row| {
            Ok(ExportRow {
                created_at: row.get(0)?,
                creator: row.get(1)?,
                kind: row.get(2)?,
                gross: row.get(3)?,
                net: row.get(4)?,
                of_transaction_id: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}

fn channel_sums(
    database: &Database,
    start: Option<i64>,
    end: Option<i64>,
    model_ids: &[i64],
    column: &str,
) -> Result<BTreeMap<String, f64>, DbError> {
    let filter = model_filter("model_id", model_ids);
    let sql = format!(
        "SELECT type, COALESCE(SUM({column}), 0)
         FROM model_earnings
         WHERE (?1 IS NULL OR creatorfloww_unix(created_at) >= ?1)
           AND (?2 IS NULL OR creatorfloww_unix(created_at) < ?2)
           {filter}
         GROUP BY type"
    );
    let mut totals = BTreeMap::new();
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![start, end], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;
        for row in rows {
            let (kind, amount) = row?;
            totals.insert(kind, amount);
        }
        Ok(())
    })?;
    Ok(totals)
}

fn metric_cards(
    current: &BTreeMap<String, f64>,
    previous: Option<(i64, i64)>,
    database: &Database,
    model_ids: &[i64],
    column: &str,
) -> Result<Vec<MetricCard>, DbError> {
    let previous_sums = match previous {
        Some((start, end)) => Some(channel_sums(database, Some(start), Some(end), model_ids, column)?),
        None => None,
    };
    let mut cards = Vec::new();
    let total = CHANNELS.iter().map(|(kind, _, _)| current.get(*kind).copied().unwrap_or(0.0)).sum();
    let previous_total = previous_sums.as_ref().map(|sums| {
        CHANNELS.iter().map(|(kind, _, _)| sums.get(*kind).copied().unwrap_or(0.0)).sum()
    });
    cards.push(MetricCard {
        key: "total".to_owned(),
        label: "Total Ganancias".to_owned(),
        amount: total,
        change_percent: previous_total.map(|previous| change_percent(total, previous)),
    });
    for (kind, key, label) in CHANNELS {
        let amount = current.get(*kind).copied().unwrap_or(0.0);
        let change_percent = previous_sums.as_ref().map(|sums| {
            change_percent(amount, sums.get(*kind).copied().unwrap_or(0.0))
        });
        cards.push(MetricCard {
            key: (*key).to_owned(),
            label: (*label).to_owned(),
            amount,
            change_percent,
        });
    }
    Ok(cards)
}

fn creator_cards(
    database: &Database,
    start: Option<i64>,
    end: Option<i64>,
    model_ids: &[i64],
    column: &str,
) -> Result<Vec<CreatorCard>, DbError> {
    let filter = model_filter("m.id", model_ids);
    let cases = CHANNELS
        .iter()
        .map(|(kind, _, _)| {
            format!("COALESCE(SUM(CASE WHEN t.type = '{kind}' THEN t.{column} ELSE 0 END), 0)")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT m.id, m.display_name, m.username, m.avatar_url, m.current_balance,
                m.pending_balance, m.last_synced_at, {cases}
         FROM models m
         LEFT JOIN model_earnings t ON t.model_id = m.id
           AND (?1 IS NULL OR creatorfloww_unix(t.created_at) >= ?1)
           AND (?2 IS NULL OR creatorfloww_unix(t.created_at) < ?2)
         WHERE 1 = 1 {filter}
         GROUP BY m.id
         ORDER BY m.display_name COLLATE NOCASE, m.id"
    );
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![start, end], |row| {
            let mut channels = Vec::new();
            let mut total = 0.0;
            for (index, (_, key, label)) in CHANNELS.iter().enumerate() {
                let amount = row.get::<_, f64>(7 + index)?;
                total += amount;
                channels.push(ChannelAmount {
                    key: (*key).to_owned(),
                    label: (*label).to_owned(),
                    amount,
                });
            }
            let display_name: String = row.get(1)?;
            let username: String = row.get(2)?;
            Ok(CreatorCard {
                id: row.get(0)?,
                name: if display_name.trim().is_empty() { username.clone() } else { display_name },
                username,
                avatar_url: row.get(3)?,
                current_balance: row.get(4)?,
                pending_balance: row.get(5)?,
                last_synced_at: row.get(6)?,
                total,
                channels,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}

fn trend(
    database: &Database,
    start: Option<i64>,
    end: Option<i64>,
    model_ids: &[i64],
    column: &str,
    bucket: TrendBucket,
) -> Result<Vec<TrendPoint>, DbError> {
    let offset = local_offset();
    let filter = model_filter("model_id", model_ids);
    let (expression, step) = match bucket {
        TrendBucket::Hour => ("((creatorfloww_unix(created_at) + ?3) / 3600) * 3600".to_owned(), Some(3600)),
        TrendBucket::Day => ("((creatorfloww_unix(created_at) + ?3) / 86400) * 86400".to_owned(), Some(86400)),
        TrendBucket::Week => ("((creatorfloww_unix(created_at) + ?3 + 259200) / 604800) * 604800 - 259200".to_owned(), Some(604800)),
        TrendBucket::Month => ("strftime('%Y-%m', datetime(creatorfloww_unix(created_at) + ?3, 'unixepoch'))".to_owned(), None),
    };
    let sql = format!(
        "SELECT {expression} AS bucket, COALESCE(SUM({column}), 0)
         FROM model_earnings
         WHERE creatorfloww_unix(created_at) IS NOT NULL
           AND (?1 IS NULL OR creatorfloww_unix(created_at) >= ?1)
           AND (?2 IS NULL OR creatorfloww_unix(created_at) < ?2)
           {filter}
         GROUP BY bucket
         ORDER BY bucket"
    );
    let mut points = BTreeMap::new();
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![start, end, offset], |row| {
            Ok((row.get::<_, rusqlite::types::Value>(0)?, row.get::<_, f64>(1)?))
        })?;
        for row in rows {
            let (bucket_value, amount) = row?;
            let label = bucket_label(bucket, &bucket_value);
            points.insert(label, amount);
        }
        Ok(())
    })?;
    if let (Some(start), Some(end), Some(step)) = (start, end, step) {
        let mut cursor = ((start + offset) / step) * step;
        let last = end + offset;
        while cursor < last {
            let label = bucket_label(bucket, &rusqlite::types::Value::Integer(cursor));
            points.entry(label).or_insert(0.0);
            cursor += step;
        }
    }
    Ok(points
        .into_iter()
        .map(|(label, amount)| TrendPoint { label, amount })
        .collect())
}

fn channel_series(
    database: &Database,
    start: Option<i64>,
    end: Option<i64>,
    model_ids: &[i64],
    column: &str,
) -> Result<Vec<ChannelPoint>, DbError> {
    let offset = local_offset();
    let filter = model_filter("model_id", model_ids);
    let sql = format!(
        "SELECT ((creatorfloww_unix(created_at) + ?3) / 86400) * 86400 AS bucket,
                COALESCE(SUM(CASE WHEN type = 'sub' THEN {column} ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN type = 'tip' THEN {column} ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN type = 'post' THEN {column} ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN type = 'message' THEN {column} ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN type = 'referral' THEN {column} ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN type = 'stream' THEN {column} ELSE 0 END), 0)
         FROM model_earnings
         WHERE creatorfloww_unix(created_at) IS NOT NULL
           AND (?1 IS NULL OR creatorfloww_unix(created_at) >= ?1)
           AND (?2 IS NULL OR creatorfloww_unix(created_at) < ?2)
           {filter}
         GROUP BY bucket
         ORDER BY bucket"
    );
    let mut points = BTreeMap::new();
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![start, end, offset], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, f64>(6)?,
            ))
        })?;
        for row in rows {
            let (bucket, subs, tips, posts, ppv, referrals, streams) = row?;
            points.insert(
                bucket,
                ChannelPoint {
                    label: day_label(bucket),
                    subs,
                    tips,
                    posts,
                    ppv,
                    referrals,
                    streams,
                },
            );
        }
        Ok(())
    })?;
    if let (Some(start), Some(end)) = (start, end) {
        let mut cursor = ((start + offset) / 86400) * 86400;
        while cursor < end + offset {
            points.entry(cursor).or_insert_with(|| ChannelPoint {
                label: day_label(cursor),
                subs: 0.0,
                tips: 0.0,
                posts: 0.0,
                ppv: 0.0,
                referrals: 0.0,
                streams: 0.0,
            });
            cursor += 86400;
        }
    }
    Ok(points.into_values().collect())
}

fn compare_daily(
    database: &Database,
    start: Option<i64>,
    end: Option<i64>,
    model_ids: &[i64],
    column: &str,
) -> Result<(Vec<String>, Vec<CreatorSeries>), DbError> {
    if model_ids.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let offset = local_offset();
    let filter = model_filter("m.id", model_ids);
    let sql = format!(
        "SELECT m.id, COALESCE(NULLIF(m.display_name, ''), m.username),
                ((creatorfloww_unix(t.created_at) + ?3) / 86400) * 86400 AS bucket,
                COALESCE(SUM(t.{column}), 0)
         FROM models m
         LEFT JOIN model_earnings t ON t.model_id = m.id
           AND creatorfloww_unix(t.created_at) IS NOT NULL
           AND (?1 IS NULL OR creatorfloww_unix(t.created_at) >= ?1)
           AND (?2 IS NULL OR creatorfloww_unix(t.created_at) < ?2)
         WHERE 1 = 1 {filter}
         GROUP BY m.id, bucket
         ORDER BY bucket, m.id"
    );
    let mut names = BTreeMap::<i64, String>::new();
    let mut values = BTreeMap::<i64, BTreeMap<i64, f64>>::new();
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![start, end, offset], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        for row in rows {
            let (id, name, bucket, amount) = row?;
            names.insert(id, name);
            if let Some(bucket) = bucket {
                values.entry(id).or_default().insert(bucket, amount);
            }
        }
        Ok(())
    })?;
    let mut labels = BTreeMap::new();
    if let (Some(start), Some(end)) = (start, end) {
        let mut cursor = ((start + offset) / 86400) * 86400;
        while cursor < end + offset {
            labels.insert(cursor, day_label(cursor));
            cursor += 86400;
        }
    } else {
        for series in values.values() {
            for bucket in series.keys() {
                labels.insert(*bucket, day_label(*bucket));
            }
        }
    }
    let label_list: Vec<String> = labels.values().cloned().collect();
    let series = model_ids
        .iter()
        .map(|id| CreatorSeries {
            id: *id,
            name: names.get(id).cloned().unwrap_or_else(|| "Unknown".to_owned()),
            values: labels
                .keys()
                .map(|bucket| values.get(id).and_then(|series| series.get(bucket)).copied().unwrap_or(0.0))
                .collect(),
        })
        .collect();
    Ok((label_list, series))
}

fn breakdown_slices(totals: &BTreeMap<String, f64>) -> Vec<BreakdownSlice> {
    let grand: f64 = CHANNELS.iter().map(|(kind, _, _)| totals.get(*kind).copied().unwrap_or(0.0)).sum();
    CHANNELS
        .iter()
        .map(|(kind, key, label)| {
            let amount = totals.get(*kind).copied().unwrap_or(0.0);
            BreakdownSlice {
                key: (*key).to_owned(),
                label: (*label).to_owned(),
                amount,
                percent: if grand == 0.0 { 0.0 } else { (amount / grand) * 100.0 },
            }
        })
        .collect()
}

fn change_percent(current: f64, previous: f64) -> f64 {
    if previous == 0.0 {
        if current == 0.0 { 0.0 } else { 100.0 }
    } else {
        ((current - previous) / previous) * 100.0
    }
}

fn bounds(range: AnalyticsRange) -> (Option<i64>, Option<i64>, Option<(i64, i64)>) {
    let now = Local::now();
    let today = now.date_naive();
    let end = crate::db::next_local_midnight();
    match range {
        AnalyticsRange::Hours24 => {
            let current = now.timestamp();
            let start = current - 86400;
            let prev_start = start - 86400;
            (Some(start), Some(current), Some((prev_start, start)))
        }
        AnalyticsRange::Days7 => {
            let start = crate::db::local_midnight(today - Duration::days(6));
            let length = end - start;
            (Some(start), Some(end), Some((start - length, start)))
        }
        AnalyticsRange::Days14 => {
            let start = crate::db::local_midnight(today - Duration::days(13));
            let length = end - start;
            (Some(start), Some(end), Some((start - length, start)))
        }
        AnalyticsRange::Days30 => {
            let start = crate::db::local_midnight(today - Duration::days(29));
            let length = end - start;
            (Some(start), Some(end), Some((start - length, start)))
        }
        AnalyticsRange::Days90 => {
            let start = crate::db::local_midnight(today - Duration::days(89));
            let length = end - start;
            (Some(start), Some(end), Some((start - length, start)))
        }
        AnalyticsRange::Days180 => {
            let start = crate::db::local_midnight(today - Duration::days(179));
            let length = end - start;
            (Some(start), Some(end), Some((start - length, start)))
        }
        AnalyticsRange::Year(year) => {
            let start_date = chrono::NaiveDate::from_ymd_opt(year, 1, 1).unwrap_or(today);
            let end_date = chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap_or(today);
            let start = crate::db::local_midnight(start_date);
            let end_ts = crate::db::local_midnight(end_date);
            let prev_date = chrono::NaiveDate::from_ymd_opt(year - 1, 1, 1).unwrap_or(today);
            let prev_start = crate::db::local_midnight(prev_date);
            (Some(start), Some(end_ts), Some((prev_start, start)))
        }
        AnalyticsRange::Month(year, month) => {
            let start_date = chrono::NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(today);
            let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
            let (prev_year, prev_month) = if month == 1 { (year - 1, 12) } else { (year, month - 1) };
            let end_date = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1).unwrap_or(today);
            let prev_date = chrono::NaiveDate::from_ymd_opt(prev_year, prev_month, 1).unwrap_or(today);
            let start = crate::db::local_midnight(start_date);
            let end_ts = crate::db::local_midnight(end_date);
            let prev_start = crate::db::local_midnight(prev_date);
            (Some(start), Some(end_ts), Some((prev_start, start)))
        }
        AnalyticsRange::All => (None, Some(end), None),
    }
}

fn local_offset() -> i64 {
    Local::now().offset().local_minus_utc().into()
}

fn model_filter(column: &str, model_ids: &[i64]) -> String {
    if model_ids.is_empty() {
        String::new()
    } else {
        let ids = model_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");
        format!(" AND {column} IN ({ids})")
    }
}

fn bucket_label(bucket: TrendBucket, value: &rusqlite::types::Value) -> String {
    match (bucket, value) {
        (TrendBucket::Month, rusqlite::types::Value::Text(text)) => text.clone(),
        (_, rusqlite::types::Value::Integer(seconds)) => match bucket {
            TrendBucket::Hour => hour_label(*seconds),
            TrendBucket::Week | TrendBucket::Day => day_label(*seconds),
            TrendBucket::Month => day_label(*seconds),
        },
        _ => String::new(),
    }
}

fn day_label(local_seconds: i64) -> String {
    let date = chrono::DateTime::from_timestamp(local_seconds, 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).expect("epoch"));
    format!("{:02}/{:02}", date.day(), date.month())
}

fn hour_label(local_seconds: i64) -> String {
    let date = chrono::DateTime::from_timestamp(local_seconds, 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).expect("epoch"));
    format!("{:02}/{:02} {:02}:00", date.day(), date.month(), date.format("%H"))
}

fn write_csv(path: &std::path::Path, rows: &[ExportRow]) -> Result<(), DbError> {
    let mut contents = String::from("created_at,creator,type,gross_amount,net_amount,of_transaction_id\n");
    for row in rows {
        contents.push_str(&format!(
            "{},{},{},{},{},{}\n",
            csv_escape(&row.created_at),
            csv_escape(&row.creator),
            csv_escape(&row.kind),
            row.gross,
            row.net,
            csv_escape(&row.of_transaction_id)
        ));
    }
    std::fs::write(path, contents)?;
    Ok(())
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn write_xlsx(path: &std::path::Path, rows: &[ExportRow]) -> Result<(), DbError> {
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let worksheet = workbook.add_worksheet();
    for (column, title) in ["Fecha", "Creadora", "Canal", "Bruto", "Neto", "ID OnlyFans"].iter().enumerate() {
        worksheet
            .write_string(0, column as u16, *title)
            .map_err(|err| DbError::Invalid(err.to_string()))?;
    }
    for (index, row) in rows.iter().enumerate() {
        let excel_row = (index + 1) as u32;
        worksheet.write_string(excel_row, 0, &row.created_at).map_err(|err| DbError::Invalid(err.to_string()))?;
        worksheet.write_string(excel_row, 1, &row.creator).map_err(|err| DbError::Invalid(err.to_string()))?;
        worksheet.write_string(excel_row, 2, &row.kind).map_err(|err| DbError::Invalid(err.to_string()))?;
        worksheet.write_number(excel_row, 3, row.gross).map_err(|err| DbError::Invalid(err.to_string()))?;
        worksheet.write_number(excel_row, 4, row.net).map_err(|err| DbError::Invalid(err.to_string()))?;
        worksheet.write_string(excel_row, 5, &row.of_transaction_id).map_err(|err| DbError::Invalid(err.to_string()))?;
    }
    workbook.save(path).map_err(|err| DbError::Invalid(err.to_string()))?;
    Ok(())
}

fn write_pdf(path: &std::path::Path, rows: &[ExportRow]) -> Result<(), DbError> {
    let mut lines = vec!["CreatorFloww — transacciones".to_owned()];
    if rows.is_empty() {
        lines.push("No data".to_owned());
    }
    for row in rows.iter().take(40) {
        lines.push(format!(
            "{}  {}  {}  {:.2} / {:.2}",
            row.created_at, row.creator, row.kind, row.gross, row.net
        ));
    }
    if rows.len() > 40 {
        lines.push(format!("… {} filas más en el archivo Excel o CSV", rows.len() - 40));
    }
    std::fs::write(path, simple_pdf(&lines))?;
    Ok(())
}

fn simple_pdf(lines: &[String]) -> Vec<u8> {
    let mut stream = String::from("BT /F1 10 Tf 40 760 Td 14 TL\n");
    for line in lines {
        let escaped = line.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
        stream.push_str(&format!("({escaped}) '\n"));
    }
    stream.push_str("ET\n");
    let objects = [
        "1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n".to_owned(),
        "2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n".to_owned(),
        "3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj\n".to_owned(),
        format!("4 0 obj << /Length {} >> stream\n{stream}endstream endobj\n", stream.len()),
        "5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n".to_owned(),
    ];
    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for object in &objects {
        offsets.push(pdf.len());
        pdf.push_str(object);
    }
    let xref = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
    for offset in offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
        objects.len() + 1
    ));
    pdf.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use chrono::{Duration, Local, LocalResult, TimeZone, Utc};
    use rusqlite::params;
    use std::sync::atomic::{AtomicU64, Ordering};

    async fn fresh() -> (Database, std::path::PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "creatorfloww-analytics-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let database = Database::initialize(dir.clone()).await.unwrap();
        (database, dir)
    }

    fn stamp(days_from_today: i64) -> String {
        let date = Local::now().date_naive() + Duration::days(days_from_today);
        let naive = date.and_hms_opt(12, 0, 0).unwrap();
        match Local.from_local_datetime(&naive) {
            LocalResult::Single(value) => value.with_timezone(&Utc),
            LocalResult::Ambiguous(earliest, _) => earliest.with_timezone(&Utc),
            LocalResult::None => naive.and_utc(),
        }
        .to_rfc3339()
    }

    #[tokio::test]
    async fn ranges_net_filter_and_comparison_stay_separate() {
        let (database, dir) = fresh().await;
        let alpha = database.create_model_partition().unwrap();
        let beta = database.create_model_partition().unwrap();
        database
            .save_onlyfans_profile(alpha.id, "1", "alpha", "Alpha", None)
            .unwrap();
        database
            .save_onlyfans_profile(beta.id, "2", "beta", "Beta", None)
            .unwrap();
        database
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO transactions (
                        of_transaction_id, model_id, type, gross_amount, net_amount, created_at
                     ) VALUES ('a-tip', ?1, 'tip', 10, 8, ?2),
                              ('b-msg', ?3, 'message', 20, 16, ?2),
                              ('a-old', ?1, 'sub', 100, 80, ?4)",
                    params![alpha.id, stamp(0), beta.id, stamp(-8)],
                )?;
                Ok(())
            })
            .unwrap();

        let overview = report(
            &database,
            AnalyticsRange::Days7,
            &[],
            &[],
            true,
            TrendBucket::Day,
        )
        .unwrap();
        let total = overview.cards.iter().find(|card| card.key == "total").unwrap();
        assert_eq!(total.amount, 24.0);
        assert!(total.change_percent.is_some());
        assert!(overview.comparison_series.is_empty());
        assert!(overview.comparison_donuts.is_empty());
        assert_eq!(overview.creators.len(), 2);
        assert!(overview.trends.iter().any(|point| point.amount == 24.0));
        assert!(overview
            .channels_over_time
            .iter()
            .any(|point| point.tips == 8.0 && point.ppv == 16.0));
        let tips = overview.breakdown.iter().find(|slice| slice.key == "tips").unwrap();
        assert!((tips.percent - (8.0 / 24.0 * 100.0)).abs() < 0.01);

        let only_alpha = report(
            &database,
            AnalyticsRange::Days7,
            &[alpha.id],
            &[],
            true,
            TrendBucket::Day,
        )
        .unwrap();
        assert_eq!(only_alpha.cards[0].amount, 8.0);
        assert_eq!(only_alpha.creators.len(), 1);

        let gross = report(
            &database,
            AnalyticsRange::Days7,
            &[],
            &[],
            false,
            TrendBucket::Hour,
        )
        .unwrap();
        assert_eq!(gross.cards[0].amount, 30.0);
        assert!(gross.trends.iter().any(|point| point.amount == 30.0));

        let compared = report(
            &database,
            AnalyticsRange::Days7,
            &[],
            &[alpha.id, beta.id],
            true,
            TrendBucket::Day,
        )
        .unwrap();
        assert_eq!(compared.cards[0].amount, 24.0);
        assert_eq!(compared.comparison_series.len(), 2);
        let alpha_series = compared
            .comparison_series
            .iter()
            .find(|series| series.id == alpha.id)
            .unwrap();
        assert_eq!(alpha_series.name, "Alpha");
        assert_eq!(alpha_series.values.iter().sum::<f64>(), 8.0);
        assert_eq!(compared.comparison_donuts.len(), 2);

        let historic = report(
            &database,
            AnalyticsRange::All,
            &[alpha.id],
            &[],
            true,
            TrendBucket::Month,
        )
        .unwrap();
        assert_eq!(historic.cards[0].amount, 88.0);
        assert!(historic.cards[0].change_percent.is_none());

        let idle = database.create_model_partition().unwrap();
        database
            .save_onlyfans_profile(idle.id, "3", "idle", "Idle", None)
            .unwrap();
        let zeros = report(
            &database,
            AnalyticsRange::Days7,
            &[],
            &[idle.id],
            true,
            TrendBucket::Day,
        )
        .unwrap();
        assert_eq!(zeros.comparison_series.len(), 1);
        assert!(zeros.comparison_series[0].values.iter().all(|value| *value == 0.0));
        assert!(zeros.comparison_donuts[0].slices.iter().all(|slice| slice.amount == 0.0));

        let rows = export_rows(&database, AnalyticsRange::Days7, &[beta.id]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].of_transaction_id, "b-msg");
        let csv_path = dir.join("out.csv");
        write_csv(&csv_path, &rows).unwrap();
        assert!(std::fs::read_to_string(&csv_path).unwrap().contains("b-msg"));
        let xlsx_path = dir.join("out.xlsx");
        write_xlsx(&xlsx_path, &rows).unwrap();
        assert!(xlsx_path.metadata().unwrap().len() > 0);
        let pdf_path = dir.join("out.pdf");
        write_pdf(&pdf_path, &rows).unwrap();
        assert!(std::fs::read(&pdf_path).unwrap().starts_with(b"%PDF"));


        let range_24h = report(
            &database,
            AnalyticsRange::Hours24,
            &[alpha.id],
            &[],
            true,
            TrendBucket::Hour,
        )
        .unwrap();
        assert_eq!(range_24h.creators.len(), 1);

        let parsed_range: AnalyticsRange = serde_json::from_str(r#""24h""#).unwrap();
        assert_eq!(parsed_range, AnalyticsRange::Hours24);
        let parsed_year: AnalyticsRange = serde_json::from_str(r#""year:2026""#).unwrap();
        assert_eq!(parsed_year, AnalyticsRange::Year(2026));
        let parsed_month: AnalyticsRange = serde_json::from_str(r#""month:2026-11""#).unwrap();
        assert_eq!(parsed_month, AnalyticsRange::Month(2026, 11));
        let parsed_90: AnalyticsRange = serde_json::from_str(r#""90""#).unwrap();
        assert_eq!(parsed_90, AnalyticsRange::Days90);
        let parsed_180: AnalyticsRange = serde_json::from_str(r#""180""#).unwrap();
        assert_eq!(parsed_180, AnalyticsRange::Days180);

        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }
}
