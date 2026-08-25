//! Rollup logic over SrumETriage's NetworkUsage/NetworkConnection rows:
//! daily byte totals (exfil volume), an hour-of-day byte fingerprint
//! ("hours of abuse"), and a session-count/duration summary.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use chrono::{DateTime, Duration, NaiveDateTime};
use serde::{Deserialize, Serialize};

/// Signed UTC offset in minutes, e.g. "-05:00" -> -300.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TzOffset(pub i32);

impl FromStr for TzOffset {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("utc") || trimmed == "Z" || trimmed == "+00:00" {
            return Ok(TzOffset(0));
        }
        let (sign, rest) = match trimmed.as_bytes().first() {
            Some(b'+') => (1i32, &trimmed[1..]),
            Some(b'-') => (-1i32, &trimmed[1..]),
            _ => return Err(format!("expected +HH:MM or -HH:MM, got {trimmed:?}")),
        };
        let (h, m) = rest
            .split_once(':')
            .ok_or_else(|| format!("expected HH:MM offset, got {trimmed:?}"))?;
        let hours: i32 = h.parse().map_err(|_| format!("invalid hour {h:?}"))?;
        let minutes: i32 = m.parse().map_err(|_| format!("invalid minute {m:?}"))?;
        if hours > 23 || minutes > 59 {
            return Err(format!("out-of-range offset {trimmed:?}"));
        }
        Ok(TzOffset(sign * (hours * 60 + minutes)))
    }
}

/// A local business-hours window used to flag off-hours activity.
/// Supports overnight windows (e.g. "22:00-06:00") via wraparound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusinessHours {
    start_minutes: u32,
    end_minutes: u32,
}

impl BusinessHours {
    pub fn contains(&self, minute_of_day: u32) -> bool {
        if self.start_minutes <= self.end_minutes {
            minute_of_day >= self.start_minutes && minute_of_day < self.end_minutes
        } else {
            minute_of_day >= self.start_minutes || minute_of_day < self.end_minutes
        }
    }
}

impl FromStr for BusinessHours {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (start, end) = s
            .split_once('-')
            .ok_or_else(|| format!("expected HH:MM-HH:MM, got {s:?}"))?;
        Ok(BusinessHours {
            start_minutes: parse_hhmm(start)?,
            end_minutes: parse_hhmm(end)?,
        })
    }
}

fn parse_hhmm(s: &str) -> Result<u32, String> {
    let (h, m) = s
        .split_once(':')
        .ok_or_else(|| format!("expected HH:MM, got {s:?}"))?;
    let hours: u32 = h.parse().map_err(|_| format!("invalid hour {h:?}"))?;
    let minutes: u32 = m.parse().map_err(|_| format!("invalid minute {m:?}"))?;
    if hours > 23 || minutes > 59 {
        return Err(format!("out-of-range time {s:?}"));
    }
    Ok(hours * 60 + minutes)
}

/// Parses SrumETriage's ISO 8601 `Timestamp` column (e.g.
/// `2024-06-29T00:05:00.1234567Z`) and shifts it by `tz`, returning the
/// resulting local naive datetime. `None` for an unparseable/blank cell —
/// callers skip the row rather than inventing a timestamp.
fn local_datetime(ts: &str, tz: TzOffset) -> Option<NaiveDateTime> {
    if ts.is_empty() {
        return None;
    }
    let parsed = DateTime::parse_from_rfc3339(ts).ok()?;
    Some(parsed.naive_utc() + Duration::minutes(tz.0 as i64))
}

// --- Input rows (subset of SrumETriage's CSV columns we need) -------------

#[derive(Debug, Deserialize)]
pub struct UsageRow {
    #[serde(rename = "Timestamp")]
    pub timestamp: String,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "BytesReceived")]
    pub bytes_received: i64,
    #[serde(rename = "BytesSent")]
    pub bytes_sent: i64,
    #[serde(rename = "InterfaceType")]
    pub interface_type: String,
    #[serde(rename = "L2ProfileId")]
    pub l2_profile_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct ConnectionRow {
    #[serde(rename = "Timestamp")]
    pub timestamp: String,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "ConnectedTime")]
    pub connected_time: i64,
}

// --- Output records ---------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DailySummaryRecord {
    #[serde(rename = "Date")]
    pub date: String,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "BytesSentTotal")]
    pub bytes_sent_total: i64,
    #[serde(rename = "BytesReceivedTotal")]
    pub bytes_received_total: i64,
    #[serde(rename = "TotalBytes")]
    pub total_bytes: i64,
    #[serde(rename = "SampleCount")]
    pub sample_count: u64,
    #[serde(rename = "DominantInterfaceType")]
    pub dominant_interface_type: String,
    #[serde(rename = "DistinctProfiles")]
    pub distinct_profiles: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HourlyFingerprintRecord {
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "HourOfDay")]
    pub hour_of_day: u32,
    #[serde(rename = "BytesSentTotal")]
    pub bytes_sent_total: i64,
    #[serde(rename = "BytesReceivedTotal")]
    pub bytes_received_total: i64,
    #[serde(rename = "SampleCount")]
    pub sample_count: u64,
    #[serde(rename = "PctOfExeBytesSent")]
    pub pct_of_exe_bytes_sent: f64,
    #[serde(rename = "OutsideBusinessHours")]
    pub outside_business_hours: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionSummaryRecord {
    #[serde(rename = "Date")]
    pub date: String,
    #[serde(rename = "ExeInfo")]
    pub exe_info: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "SessionCount")]
    pub session_count: u64,
    #[serde(rename = "TotalConnectedSeconds")]
    pub total_connected_seconds: i64,
}

// --- Aggregation --------------------------------------------------------

#[derive(Default)]
struct DailyAgg {
    bytes_sent_total: i64,
    bytes_received_total: i64,
    sample_count: u64,
    interface_counts: BTreeMap<String, u64>,
    profiles: BTreeSet<i64>,
}

pub fn aggregate_daily(rows: &[UsageRow], tz: TzOffset) -> Vec<DailySummaryRecord> {
    let mut groups: BTreeMap<(String, String, String), DailyAgg> = BTreeMap::new();
    for row in rows {
        let Some(local) = local_datetime(&row.timestamp, tz) else {
            continue;
        };
        let date = local.date().format("%Y-%m-%d").to_string();
        let key = (row.exe_info.clone(), row.user_name.clone(), date);
        let agg = groups.entry(key).or_default();
        agg.bytes_sent_total += row.bytes_sent;
        agg.bytes_received_total += row.bytes_received;
        agg.sample_count += 1;
        *agg.interface_counts
            .entry(row.interface_type.clone())
            .or_insert(0) += 1;
        agg.profiles.insert(row.l2_profile_id);
    }

    let mut out: Vec<DailySummaryRecord> = groups
        .into_iter()
        .map(|((exe_info, user_name, date), agg)| {
            let dominant_interface_type = agg
                .interface_counts
                .iter()
                .max_by_key(|(_, count)| **count)
                .map(|(name, _)| name.clone())
                .unwrap_or_default();
            DailySummaryRecord {
                date,
                exe_info,
                user_name,
                bytes_sent_total: agg.bytes_sent_total,
                bytes_received_total: agg.bytes_received_total,
                total_bytes: agg.bytes_sent_total + agg.bytes_received_total,
                sample_count: agg.sample_count,
                dominant_interface_type,
                distinct_profiles: agg.profiles.len() as u64,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.bytes_sent_total
            .cmp(&a.bytes_sent_total)
            .then_with(|| a.exe_info.cmp(&b.exe_info))
            .then_with(|| a.date.cmp(&b.date))
    });
    out
}

#[derive(Default)]
struct HourlyAgg {
    bytes_sent_total: i64,
    bytes_received_total: i64,
    sample_count: u64,
}

pub fn aggregate_hourly(
    rows: &[UsageRow],
    tz: TzOffset,
    business_hours: BusinessHours,
) -> Vec<HourlyFingerprintRecord> {
    let mut groups: BTreeMap<(String, String, u32), HourlyAgg> = BTreeMap::new();
    let mut exe_totals: BTreeMap<String, i64> = BTreeMap::new();
    for row in rows {
        let Some(local) = local_datetime(&row.timestamp, tz) else {
            continue;
        };
        let hour = local
            .time()
            .format("%H")
            .to_string()
            .parse::<u32>()
            .unwrap_or(0);
        let key = (row.exe_info.clone(), row.user_name.clone(), hour);
        let agg = groups.entry(key).or_default();
        agg.bytes_sent_total += row.bytes_sent;
        agg.bytes_received_total += row.bytes_received;
        agg.sample_count += 1;
        *exe_totals.entry(row.exe_info.clone()).or_insert(0) += row.bytes_sent;
    }

    let mut out: Vec<HourlyFingerprintRecord> = groups
        .into_iter()
        .map(|((exe_info, user_name, hour_of_day), agg)| {
            let exe_total = exe_totals.get(&exe_info).copied().unwrap_or(0);
            let pct_of_exe_bytes_sent = if exe_total > 0 {
                agg.bytes_sent_total as f64 / exe_total as f64
            } else {
                0.0
            };
            let minute_of_day = hour_of_day * 60;
            HourlyFingerprintRecord {
                exe_info,
                user_name,
                hour_of_day,
                bytes_sent_total: agg.bytes_sent_total,
                bytes_received_total: agg.bytes_received_total,
                sample_count: agg.sample_count,
                pct_of_exe_bytes_sent,
                outside_business_hours: !business_hours.contains(minute_of_day),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.outside_business_hours
            .cmp(&a.outside_business_hours)
            .then_with(|| {
                b.pct_of_exe_bytes_sent
                    .partial_cmp(&a.pct_of_exe_bytes_sent)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.exe_info.cmp(&b.exe_info))
    });
    out
}

#[derive(Default)]
struct SessionAgg {
    session_count: u64,
    total_connected_seconds: i64,
}

pub fn aggregate_sessions(rows: &[ConnectionRow], tz: TzOffset) -> Vec<SessionSummaryRecord> {
    let mut groups: BTreeMap<(String, String, String), SessionAgg> = BTreeMap::new();
    for row in rows {
        let Some(local) = local_datetime(&row.timestamp, tz) else {
            continue;
        };
        let date = local.date().format("%Y-%m-%d").to_string();
        let key = (row.exe_info.clone(), row.user_name.clone(), date);
        let agg = groups.entry(key).or_default();
        agg.session_count += 1;
        agg.total_connected_seconds += row.connected_time;
    }

    let mut out: Vec<SessionSummaryRecord> = groups
        .into_iter()
        .map(|((exe_info, user_name, date), agg)| SessionSummaryRecord {
            date,
            exe_info,
            user_name,
            session_count: agg.session_count,
            total_connected_seconds: agg.total_connected_seconds,
        })
        .collect();
    out.sort_by(|a, b| {
        b.total_connected_seconds
            .cmp(&a.total_connected_seconds)
            .then_with(|| a.exe_info.cmp(&b.exe_info))
            .then_with(|| a.date.cmp(&b.date))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage_row(ts: &str, exe: &str, user: &str, sent: i64, recvd: i64) -> UsageRow {
        UsageRow {
            timestamp: ts.into(),
            exe_info: exe.into(),
            user_name: user.into(),
            bytes_received: recvd,
            bytes_sent: sent,
            interface_type: "Wired80211".into(),
            l2_profile_id: 1,
        }
    }

    #[test]
    fn tz_offset_parses_signed_hhmm() {
        assert_eq!("+02:00".parse::<TzOffset>().unwrap(), TzOffset(120));
        assert_eq!("-05:30".parse::<TzOffset>().unwrap(), TzOffset(-330));
        assert_eq!("UTC".parse::<TzOffset>().unwrap(), TzOffset(0));
        assert!("garbage".parse::<TzOffset>().is_err());
    }

    #[test]
    fn business_hours_flags_default_window() {
        let bh: BusinessHours = "08:00-18:00".parse().unwrap();
        assert!(bh.contains(9 * 60));
        assert!(!bh.contains(2 * 60));
        assert!(!bh.contains(18 * 60));
    }

    #[test]
    fn business_hours_supports_overnight_wraparound() {
        let bh: BusinessHours = "22:00-06:00".parse().unwrap();
        assert!(bh.contains(23 * 60));
        assert!(bh.contains(60));
        assert!(!bh.contains(12 * 60));
    }

    #[test]
    fn daily_aggregation_sums_bytes_per_exe_user_day() {
        let rows = vec![
            usage_row(
                "2024-06-29T00:05:00.0000000Z",
                "chrome.exe",
                "alice",
                100,
                50,
            ),
            usage_row(
                "2024-06-29T05:00:00.0000000Z",
                "chrome.exe",
                "alice",
                200,
                20,
            ),
            usage_row(
                "2024-06-30T00:00:00.0000000Z",
                "chrome.exe",
                "alice",
                10,
                10,
            ),
        ];
        let out = aggregate_daily(&rows, TzOffset(0));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].date, "2024-06-29");
        assert_eq!(out[0].bytes_sent_total, 300);
        assert_eq!(out[0].bytes_received_total, 70);
        assert_eq!(out[0].total_bytes, 370);
        assert_eq!(out[0].sample_count, 2);
    }

    #[test]
    fn daily_aggregation_sorts_by_bytes_sent_descending() {
        let rows = vec![
            usage_row("2024-06-29T00:00:00.0000000Z", "quiet.exe", "alice", 10, 0),
            usage_row("2024-06-29T00:00:00.0000000Z", "loud.exe", "alice", 9000, 0),
        ];
        let out = aggregate_daily(&rows, TzOffset(0));
        assert_eq!(out[0].exe_info, "loud.exe");
        assert_eq!(out[1].exe_info, "quiet.exe");
    }

    #[test]
    fn tz_offset_shifts_calendar_day_boundary() {
        // 23:30 UTC + 2h local = 01:30 next day locally.
        let rows = vec![usage_row(
            "2024-06-29T23:30:00.0000000Z",
            "chrome.exe",
            "alice",
            100,
            0,
        )];
        let utc = aggregate_daily(&rows, TzOffset(0));
        let plus2 = aggregate_daily(&rows, TzOffset(120));
        assert_eq!(utc[0].date, "2024-06-29");
        assert_eq!(plus2[0].date, "2024-06-30");
    }

    #[test]
    fn hourly_fingerprint_flags_off_hours_and_normalizes_by_exe_total() {
        let rows = vec![
            // day-time, business hours
            usage_row(
                "2024-06-29T10:00:00.0000000Z",
                "beacon.exe",
                "alice",
                100,
                0,
            ),
            // off-hours spike, same exe
            usage_row(
                "2024-06-30T02:00:00.0000000Z",
                "beacon.exe",
                "alice",
                900,
                0,
            ),
        ];
        let bh: BusinessHours = "08:00-18:00".parse().unwrap();
        let out = aggregate_hourly(&rows, TzOffset(0), bh);
        assert_eq!(out.len(), 2);
        // off-hours row ranks first: OutsideBusinessHours desc, then pct desc.
        assert_eq!(out[0].hour_of_day, 2);
        assert!(out[0].outside_business_hours);
        assert!((out[0].pct_of_exe_bytes_sent - 0.9).abs() < 1e-9);
        assert_eq!(out[1].hour_of_day, 10);
        assert!(!out[1].outside_business_hours);
    }

    #[test]
    fn session_aggregation_sums_connected_time_and_counts_sessions() {
        let rows = vec![
            ConnectionRow {
                timestamp: "2024-06-29T01:00:00.0000000Z".into(),
                exe_info: "chrome.exe".into(),
                user_name: "alice".into(),
                connected_time: 300,
            },
            ConnectionRow {
                timestamp: "2024-06-29T02:00:00.0000000Z".into(),
                exe_info: "chrome.exe".into(),
                user_name: "alice".into(),
                connected_time: 120,
            },
        ];
        let out = aggregate_sessions(&rows, TzOffset(0));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].session_count, 2);
        assert_eq!(out[0].total_connected_seconds, 420);
    }

    #[test]
    fn rows_with_unparseable_timestamps_are_skipped() {
        let rows = vec![usage_row("", "chrome.exe", "alice", 100, 0)];
        assert!(aggregate_daily(&rows, TzOffset(0)).is_empty());
    }
}
