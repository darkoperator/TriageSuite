//! The case's own settings, kept in `ox.setting`: for now the display zone,
//! the IANA time zone the case's UTC times are shown in. Storage and export
//! stay UTC whatever it is.

use super::{Case, CASE_FORMAT, FORMAT_2};
use crate::actor::Actor;
use crate::annot::{record, Change};
use crate::error::{EngineError, Result};
use crate::time::now_iso;
use duckdb::{params, Connection};
use triage_core::timestamp::zone;

/// The display zone of a case that never set one.
const DEFAULT_ZONE: &str = "UTC";

/// `ox.setting`'s key for the display zone.
const DISPLAY_ZONE: &str = "display_zone";

/// The longest zone text an error message repeats.
const SHOWN_MAX: usize = 64;

/// The case's display zone: `UTC` when it was never set, and for a
/// format-1 case, which has no settings. `Internal` when the case holds a
/// zone this build does not know, which the engine never writes.
pub fn display_zone(case: &Case) -> Result<String> {
    if case.format() < FORMAT_2 {
        return Ok(DEFAULT_ZONE.to_string());
    }
    stored_zone(case.conn())
}

/// The display zone `ox.setting` holds on `conn`, `UTC` when it holds none:
/// [`display_zone`] for a connection or transaction of a case at format 2 or
/// later (one with records always is). `Internal` as `display_zone` is.
pub(crate) fn stored_zone(conn: &Connection) -> Result<String> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM ox.setting WHERE key = ?",
            params![DISPLAY_ZONE],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            duckdb::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    match stored {
        None => Ok(DEFAULT_ZONE.to_string()),
        Some(name) if zone::is_zone(&name) => Ok(name),
        Some(name) => Err(EngineError::Internal(format!(
            "the case's display zone {} is not a time zone this build knows",
            shown(&name)
        ))),
    }
}

/// Set the case's display zone to `zone_name`, an IANA zone name spelled
/// exactly (`America/New_York`, `UTC`). `Refused` for an agent: the zone
/// changes every time the analyst reads, so only an analyst sets it.
/// `Invalid` for a name this build does not know. Setting the zone already
/// in effect changes nothing and records nothing; otherwise one history
/// row, `display_zone_set` on the case, says what it was and what it is.
pub fn set_display_zone(case: &mut Case, zone_name: &str, actor: &Actor) -> Result<()> {
    if actor.is_agent() {
        return Err(EngineError::Refused(
            "an agent may not change the display zone; an analyst must".to_string(),
        ));
    }
    if !zone::is_zone(zone_name) {
        return Err(EngineError::Invalid(format!(
            "{} is not a time zone this build knows; a zone is an IANA name spelled \
             exactly, for example America/New_York or UTC",
            shown(zone_name)
        )));
    }
    if case.format() < CASE_FORMAT {
        return Err(EngineError::Refused(format!(
            "the case is format {}, opened read-only; open it to change it, which \
             upgrades it to format {CASE_FORMAT}",
            case.format()
        )));
    }
    let current = display_zone(case)?;
    if current == zone_name {
        return Ok(());
    }
    let case_id = case.case_id()?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    tx.execute(
        "INSERT INTO ox.setting (key, value, set_by, set_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT (key) DO UPDATE SET \
           value = excluded.value, set_by = excluded.set_by, set_at = excluded.set_at",
        params![DISPLAY_ZONE, zone_name, actor.label(), at],
    )?;
    record(
        &tx,
        actor,
        &at,
        Change {
            target_kind: "case",
            target_id: case_id,
            action: "display_zone_set",
            before: Some(serde_json::json!({ DISPLAY_ZONE: current })),
            after: Some(serde_json::json!({ DISPLAY_ZONE: zone_name })),
        },
    )?;
    tx.commit()?;
    Ok(())
}

/// `text` quoted for a message, cut to `SHOWN_MAX` characters.
fn shown(text: &str) -> String {
    if text.chars().count() <= SHOWN_MAX {
        format!("{text:?}")
    } else {
        let cut: String = text.chars().take(SHOWN_MAX).collect();
        format!("{cut:?}...")
    }
}
