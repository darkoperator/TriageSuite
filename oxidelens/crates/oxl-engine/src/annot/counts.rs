//! How many rows in the case's views carry each mark, for the counts the
//! workbench keeps in sight: in all, and per relation, which the left pane
//! colours each dataset's name by (ruling S12).

use super::{locate_rows, Standing};
use crate::case::Case;
use crate::error::Result;
use crate::query::{read_mark, MarkState};
use crate::record::marks_relation;
use std::collections::{BTreeMap, HashMap};

/// Marked rows that are in their relation's view, by the mark
/// `ox.row_state` gives them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MarkCounts {
    pub suspicious: u64,
    pub malicious: u64,
    pub agent_malicious: u64,
    /// Each relation with a marked row in its view, by name.
    pub by_relation: Vec<RelationMarks>,
}

/// One relation's marked rows in its view.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RelationMarks {
    pub relation: String,
    pub suspicious: u64,
    pub malicious: u64,
    pub agent_malicious: u64,
}

impl RelationMarks {
    fn add(&mut self, mark: MarkState) {
        match mark {
            MarkState::Suspicious => self.suspicious += 1,
            MarkState::Malicious => self.malicious += 1,
            MarkState::AgentMalicious => self.agent_malicious += 1,
        }
    }
}

/// The marked rows that are in their relation's view, by mark and by
/// relation: a mark left behind on a superseded row, or on a removed
/// source's, is not counted (`marks` still lists it, with `active` false).
/// `Internal` for a mark the engine does not know.
pub fn mark_counts(case: &Case) -> Result<MarkCounts> {
    let conn = case.conn();
    let sql = format!(
        "SELECT row_id, max(mark) FROM {} AS s GROUP BY row_id",
        marks_relation(case)
    );
    let marked: HashMap<i64, Option<String>> = conn
        .prepare(&sql)?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let ids: Vec<i64> = marked.keys().copied().collect();
    let located = locate_rows(conn, &ids)?;
    let mut by_relation: BTreeMap<String, RelationMarks> = BTreeMap::new();
    for (row_id, mark) in marked {
        let Some(row) = located.get(&row_id) else {
            continue;
        };
        if row.standing != Standing::Active {
            continue;
        }
        let Some(mark) = read_mark(mark)? else {
            continue;
        };
        by_relation
            .entry(row.relation.clone())
            .or_insert_with(|| RelationMarks {
                relation: row.relation.clone(),
                suspicious: 0,
                malicious: 0,
                agent_malicious: 0,
            })
            .add(mark);
    }
    let by_relation: Vec<RelationMarks> = by_relation.into_values().collect();
    let total = |of: fn(&RelationMarks) -> u64| by_relation.iter().map(of).sum();
    Ok(MarkCounts {
        suspicious: total(|r| r.suspicious),
        malicious: total(|r| r.malicious),
        agent_malicious: total(|r| r.agent_malicious),
        by_relation,
    })
}
