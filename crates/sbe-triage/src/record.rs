//! The SBECmd shellbag record — one row per BagMRU node. Field order IS the
//! 19-column CSV order; the OutputRouter serializes to CSV and NDJSON. All
//! columns are String (uniform CSV/NDJSON rendering; empty = absent, matching
//! SBECmd; HasExplored is the .NET-bool literal "True"/"False").

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ShellbagRecord {
    #[serde(rename = "BagPath")]
    pub bag_path: String,
    #[serde(rename = "Slot")]
    pub slot: String,
    #[serde(rename = "NodeSlot")]
    pub node_slot: String,
    #[serde(rename = "MRUPosition")]
    pub mru_position: String,
    #[serde(rename = "AbsolutePath")]
    pub absolute_path: String,
    #[serde(rename = "ShellType")]
    pub shell_type: String,
    #[serde(rename = "Value")]
    pub value: String,
    #[serde(rename = "ChildBags")]
    pub child_bags: String,
    #[serde(rename = "CreatedOn")]
    pub created_on: String,
    #[serde(rename = "ModifiedOn")]
    pub modified_on: String,
    #[serde(rename = "AccessedOn")]
    pub accessed_on: String,
    #[serde(rename = "LastWriteTime")]
    pub last_write_time: String,
    #[serde(rename = "MFTEntry")]
    pub mft_entry: String,
    #[serde(rename = "MFTSequenceNumber")]
    pub mft_sequence_number: String,
    #[serde(rename = "ExtensionBlockCount")]
    pub extension_block_count: String,
    #[serde(rename = "FirstInteracted")]
    pub first_interacted: String,
    #[serde(rename = "LastInteracted")]
    pub last_interacted: String,
    #[serde(rename = "HasExplored")]
    pub has_explored: String,
    #[serde(rename = "Miscellaneous")]
    pub miscellaneous: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_header_matches_sbecmd() {
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(ShellbagRecord {
            bag_path: "BagMRU".into(),
            slot: "0".into(),
            node_slot: "0".into(),
            mru_position: "1".into(),
            absolute_path: "Desktop".into(),
            shell_type: "Root folder: GUID".into(),
            value: "x".into(),
            child_bags: "0".into(),
            created_on: String::new(),
            modified_on: String::new(),
            accessed_on: String::new(),
            last_write_time: String::new(),
            mft_entry: String::new(),
            mft_sequence_number: String::new(),
            extension_block_count: "0".into(),
            first_interacted: String::new(),
            last_interacted: String::new(),
            has_explored: "False".into(),
            miscellaneous: String::new(),
        })
        .unwrap();
        let out = String::from_utf8(w.into_inner().unwrap()).unwrap();
        let header = out.lines().next().unwrap();
        assert_eq!(header, "BagPath,Slot,NodeSlot,MRUPosition,AbsolutePath,ShellType,Value,ChildBags,CreatedOn,ModifiedOn,AccessedOn,LastWriteTime,MFTEntry,MFTSequenceNumber,ExtensionBlockCount,FirstInteracted,LastInteracted,HasExplored,Miscellaneous");
    }
}
