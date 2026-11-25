use chrono::{Datelike, Duration, Local, NaiveDate};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::{Database, DbError};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DashboardRange {
    Yesterday,
    Today,
    ThisWeek,
    ThisMonth,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EarningsBasis {
    Net,
    Gross,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelTotal {
    pub key: &'static str,
    pub label: &'static str,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EarningsOverview {
    pub total: f64,
    pub channels: Vec<ChannelTotal>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShiftRow {
    pub id: i64,
    pub employee_name: String,
    pub role: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockedInEmployee {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub last_clock_in: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmployeeSale {
    pub employee_id: i64,
    pub name: String,
    pub amount: f64,
}

const CHANNELS: &[(&str, &str, &str)] = &[
    ("sub", "subscriptions", "Subscriptions"),
    ("post", "posts", "Posts"),
    ("message", "messages", "Messages"),
    ("tip", "tips", "Tips"),
    ("referral", "referrals", "Referrals"),
    ("stream", "streams", "Streams"),
];

pub fn earnings_overview(
    database: &Database,
    range: DashboardRange,
    basis: EarningsBasis,
) -> Result<EarningsOverview, DbError> {
    let (start, end) = range_bounds(range);
    let column = amount_column(basis);
    let sql = format!(
        "SELECT type, COALESCE(SUM({column}), 0)
         FROM model_earnings
         WHERE {CREATED} >= ?1 AND {CREATED} < ?2
         GROUP BY type"
    );
    let mut totals = std::collections::BTreeMap::<String, f64>::new();
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
    let channels = CHANNELS
        .iter()
        .map(|(db_type, key, label)| ChannelTotal {
            key,
            label,
            amount: totals.get(*db_type).copied().unwrap_or(0.0),
        })
        .collect::<Vec<_>>();
    let total = channels.iter().map(|channel| channel.amount).sum();
    Ok(EarningsOverview { total, channels })
}

pub fn my_shifts(
    database: &Database,
    range: DashboardRange,
    basis: EarningsBasis,
) -> Result<Vec<ShiftRow>, DbError> {
    let (start, end) = range_bounds(range);
    let column = amount_column(basis);
    let sql = format!(
        "SELECT s.id, e.name, e.role, s.start_time, s.end_time,
                COALESCE((
                    SELECT SUM(t.{column})
                    FROM transactions t
                    WHERE t.employee_id = s.employee_id
                      AND t.type IN ('message', 'tip')
                      AND {TX} >= {SHIFT_START}
                      AND (s.end_time IS NULL OR {TX} <= {SHIFT_END})
                      AND {TX} >= ?1 AND {TX} < ?2
                ), 0)
         FROM shifts s
         JOIN employees e ON e.id = s.employee_id
         WHERE {SHIFT_START} < ?2
           AND (s.end_time IS NULL OR {SHIFT_END} >= ?1)
         ORDER BY s.start_time DESC, s.id DESC"
    );
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![start, end], |row| {
            Ok(ShiftRow {
                id: row.get(0)?,
                employee_name: row.get(1)?,
                role: row.get(2)?,
                start_time: row.get(3)?,
                end_time: row.get(4)?,
                amount: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}

pub fn clocked_in_employees(database: &Database) -> Result<Vec<ClockedInEmployee>, DbError> {
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, role, last_clock_in
             FROM employees
             WHERE is_clocked_in = 1
             ORDER BY name COLLATE NOCASE, id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ClockedInEmployee {
                id: row.get(0)?,
                name: row.get(1)?,
                role: row.get(2)?,
                last_clock_in: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}

pub fn employee_sales(
    database: &Database,
    range: DashboardRange,
    basis: EarningsBasis,
) -> Result<Vec<EmployeeSale>, DbError> {
    let (start, end) = range_bounds(range);
    let column = amount_column(basis);
    let sql = format!(
        "SELECT e.id, e.name, COALESCE(SUM(t.{column}), 0)
         FROM transactions t
         JOIN employees e ON e.id = t.employee_id
         WHERE t.type IN ('message', 'tip')
           AND {TX} >= ?1 AND {TX} < ?2
         GROUP BY e.id, e.name
         ORDER BY e.name COLLATE NOCASE, e.id"
    );
    database.with_connection(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![start, end], |row| {
            Ok(EmployeeSale {
                employee_id: row.get(0)?,
                name: row.get(1)?,
                amount: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}

const CREATED: &str = "creatorfloww_unix(created_at)";
const TX: &str = "creatorfloww_unix(t.created_at)";
const SHIFT_START: &str = "creatorfloww_unix(s.start_time)";
const SHIFT_END: &str = "creatorfloww_unix(s.end_time)";

fn amount_column(basis: EarningsBasis) -> &'static str {
    match basis {
        EarningsBasis::Net => "net_amount",
        EarningsBasis::Gross => "gross_amount",
    }
}

fn range_bounds(range: DashboardRange) -> (i64, i64) {
    let today = Local::now().date_naive();
    let (start, end) = match range {
        DashboardRange::Today => (today, today + Duration::days(1)),
        DashboardRange::Yesterday => (today - Duration::days(1), today),
        DashboardRange::ThisWeek => {
            let start = today - Duration::days(today.weekday().num_days_from_monday() as i64);
            (start, start + Duration::days(7))
        }
        DashboardRange::ThisMonth => {
            let start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
                .unwrap_or(today);
            let end = if today.month() == 12 {
                NaiveDate::from_ymd_opt(today.year() + 1, 1, 1).unwrap_or(start)
            } else {
                NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1).unwrap_or(start)
            };
            (start, end)
        }
    };
    let start = crate::db::local_midnight(start);
    let end = crate::db::local_midnight(end).min(crate::db::next_local_midnight());
    (start, end.max(start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, EmployeeRole, TransactionType};
    use chrono::{Duration, Local, LocalResult, TimeZone, Utc};
    use rusqlite::params;

    async fn fresh() -> (Database, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "creatorfloww-dashboard-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let database = Database::initialize(dir.clone()).await.unwrap();
        (database, dir)
    }

    fn stamp(days_from_today: i64, hour: u32) -> String {
        let date = Local::now().date_naive() + Duration::days(days_from_today);
        let naive = date.and_hms_opt(hour, 15, 0).unwrap();
        match Local.from_local_datetime(&naive) {
            LocalResult::Single(value) => value.with_timezone(&Utc),
            LocalResult::Ambiguous(earliest, _) => earliest.with_timezone(&Utc),
            LocalResult::None => naive.and_utc(),
        }
        .to_rfc3339()
    }

    #[tokio::test]
    async fn sums_only_the_selected_local_day_and_basis() {
        let (database, dir) = fresh().await;
        let model = database.create_model_partition().unwrap();
        let today = stamp(0, 12);
        let yesterday = stamp(-1, 12);
        database
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO transactions (
                        of_transaction_id, model_id, type, gross_amount, net_amount, created_at
                     ) VALUES ('today-tip', ?1, 'tip', 10, 8, ?2),
                              ('yesterday-sub', ?1, 'sub', 5, 4, ?3)",
                    params![model.id, today, yesterday],
                )?;
                Ok(())
            })
            .unwrap();

        let today_net = earnings_overview(&database, DashboardRange::Today, EarningsBasis::Net).unwrap();
        assert_eq!(today_net.total, 8.0);
        assert_eq!(
            today_net
                .channels
                .iter()
                .find(|channel| channel.key == "tips")
                .unwrap()
                .amount,
            8.0
        );
        assert!(today_net.channels.iter().all(|channel| channel.key != "tips" || channel.amount == 8.0));
        assert_eq!(today_net.channels.len(), 6);

        let today_gross =
            earnings_overview(&database, DashboardRange::Today, EarningsBasis::Gross).unwrap();
        assert_eq!(today_gross.total, 10.0);

        let yesterday_net =
            earnings_overview(&database, DashboardRange::Yesterday, EarningsBasis::Net).unwrap();
        assert_eq!(yesterday_net.total, 4.0);
        assert_eq!(
            yesterday_net
                .channels
                .iter()
                .find(|channel| channel.key == "subscriptions")
                .unwrap()
                .amount,
            4.0
        );

        let week = earnings_overview(&database, DashboardRange::ThisWeek, EarningsBasis::Net).unwrap();
        let month = earnings_overview(&database, DashboardRange::ThisMonth, EarningsBasis::Net).unwrap();
        assert!(week.total >= 8.0);
        assert!(month.total >= 8.0);
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn shifts_clocked_in_and_employee_sales_follow_the_rows() {
        let (database, dir) = fresh().await;
        let model = database.create_model_partition().unwrap();
        let employee = database
            .add_employee("Ana", EmployeeRole::Chatter, &serde_json::json!({}))
            .unwrap();
        database.toggle_clock_in(employee.id).unwrap();
        let created_at = (Utc::now() + Duration::seconds(2)).to_rfc3339();
        database
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO transactions (
                        of_transaction_id, model_id, employee_id, type, gross_amount, net_amount, created_at
                     ) VALUES ('sale-1', ?1, ?2, ?3, 12.5, 10, ?4)",
                    params![model.id, employee.id, TransactionType::Message.as_str(), created_at],
                )?;
                Ok(())
            })
            .unwrap();

        let clocked = clocked_in_employees(&database).unwrap();
        assert_eq!(clocked.len(), 1);
        assert_eq!(clocked[0].name, "Ana");

        let sales = employee_sales(&database, DashboardRange::Today, EarningsBasis::Net).unwrap();
        assert_eq!(sales.len(), 1);
        assert_eq!(sales[0].amount, 10.0);

        let shifts = my_shifts(&database, DashboardRange::Today, EarningsBasis::Gross).unwrap();
        assert_eq!(shifts.len(), 1);
        assert_eq!(shifts[0].employee_name, "Ana");
        assert_eq!(shifts[0].amount, 12.5);
        assert!(shifts[0].end_time.is_none());

        let yesterday_sales =
            employee_sales(&database, DashboardRange::Yesterday, EarningsBasis::Net).unwrap();
        assert!(yesterday_sales.is_empty());
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn month_and_history_skip_future_rows_and_keep_one_local_day() {
        let (database, dir) = fresh().await;
        let model = database.create_model_partition().unwrap();
        let noon = stamp(0, 12);
        let evening = stamp(0, 21);
        let later = stamp(4, 12);
        let day = Local::now().date_naive().format("%Y-%m-%d");
        database
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO transactions (
                        of_transaction_id, model_id, type, gross_amount, net_amount, created_at
                     ) VALUES (?1, ?2, 'message', 10, 8, ?3),
                              ('chat-msg-evening', ?2, 'message', 10, 8, ?4),
                              ('later-msg', ?2, 'message', 27.39, 21.91, ?5)",
                    params![
                        format!("chart:{0}:message:{day}", model.id),
                        model.id,
                        noon,
                        evening,
                        later
                    ],
                )?;
                Ok(())
            })
            .unwrap();

        let month = earnings_overview(&database, DashboardRange::ThisMonth, EarningsBasis::Net).unwrap();
        assert_eq!(month.total, 8.0);
        let history = crate::analytics::report(
            &database,
            crate::analytics::AnalyticsRange::All,
            &[],
            &[],
            true,
            crate::analytics::TrendBucket::Day,
        )
        .unwrap();
        let total = history.cards.iter().find(|card| card.key == "total").unwrap();
        assert_eq!(total.amount, 8.0);
        let messages = month.channels.iter().find(|channel| channel.key == "messages").unwrap();
        assert_eq!(messages.amount, 8.0);
        drop(database);
        let _ = std::fs::remove_dir_all(dir);
    }
}
