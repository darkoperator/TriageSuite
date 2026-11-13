//! Each host's own Windows time zone, as RETriage read it from the SYSTEM
//! hive: what the app's zone picker offers beside UTC and the case's zone.
//!
//! The zone is the `TimeZoneKeyName` value of
//! `retriage_timezoneinfo_system`, a Windows zone name, mapped to its IANA
//! name through `triage_core::timestamp::zone::windows_to_iana` (CLDR). A
//! name CLDR does not map is reported as written with no IANA name: a zone
//! is never guessed.

use crate::case::Case;
use crate::catalog::ColumnRole;
use crate::error::{EngineError, Result};
use crate::query::{scope, GridQuery, ROW_ALIAS};
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
use triage_core::output::duckdb::sql::quote_ident;
use triage_core::timestamp::zone::windows_to_iana;

/// The relation RETriage writes the SYSTEM hive's `TimeZoneInformation`
/// key to.
const RELATION: &str = "retriage_timezoneinfo_system";
/// Its data columns: the registry value's name and its data.
const NAME_COLUMN: &str = "ValueName";
const DATA_COLUMN: &str = "ValueData";
/// The value holding the zone's Windows name.
const KEY_NAME: &str = "TimeZoneKeyName";

/// One host's own time zone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HostZone {
    pub host: String,
    /// The zone's Windows name as the registry holds it
    /// (`Eastern Standard Time`).
    pub windows_name: String,
    /// Its IANA name (`America/New_York`); `None` when CLDR maps no zone of
    /// that name.
    pub iana: Option<String>,
}

/// Every host's `TimeZoneKeyName` among the active rows of
/// `retriage_timezoneinfo_system`, sorted by host, then name; one entry per
/// distinct pair. Empty when the case has no such relation, or it lacks the
/// columns or a host column.
pub fn host_zones(case: &Case) -> Result<Vec<HostZone>> {
    let q = GridQuery {
        relation: RELATION.to_string(),
        ..GridQuery::default()
    };
    // With no hosts and no columns named, the only `NotFound` is the
    // relation's own.
    let scope = match scope(case, &q) {
        Ok(scope) => scope,
        Err(EngineError::NotFound(_)) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let data = |name: &str| {
        scope
            .all
            .iter()
            .find(|c| c.name == name && c.role == ColumnRole::Data)
            .map(|c| format!("{ROW_ALIAS}.{}", quote_ident(&c.text_column)))
    };
    let (Some(name), Some(value), Some(host)) = (
        data(NAME_COLUMN),
        data(DATA_COLUMN),
        scope.relation.host_column.as_deref(),
    ) else {
        return Ok(Vec::new());
    };
    let host = format!("{ROW_ALIAS}.{}", quote_ident(host));
    let sql = format!(
        "SELECT DISTINCT CAST({host} AS VARCHAR) AS h, CAST({value} AS VARCHAR) AS v \
         FROM {from} WHERE ({where_sql}) AND CAST({name} AS VARCHAR) = ? \
           AND {host} IS NOT NULL AND {value} IS NOT NULL \
         ORDER BY h, v",
        from = scope.from_sql,
        where_sql = scope.where_sql,
    );
    let mut params = scope.params;
    params.push(SqlValue::Text(KEY_NAME.to_string()));
    let rows: Vec<(String, String)> = case
        .conn()
        .prepare(&sql)?
        .query_map(params_from_iter(params), |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows
        .into_iter()
        .map(|(host, windows_name)| HostZone {
            iana: windows_to_iana(windows_name.trim()).map(str::to_string),
            host,
            windows_name,
        })
        .collect())
}
