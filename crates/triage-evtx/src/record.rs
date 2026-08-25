use serde::Serialize;
use std::path::PathBuf;

/// One output row. Field order is the column order, and `#[serde(rename)]`
/// supplies the header names — both chosen to match EvtxECmd's CSV exactly
/// (27 columns, same order). JSON output uses the same renamed keys.
#[derive(Debug, Clone, Serialize)]
pub struct EventRecord {
    #[serde(rename = "RecordNumber")]
    pub record_number: u64,
    #[serde(rename = "EventRecordId")]
    pub event_record_id: u64,
    #[serde(rename = "TimeCreated")]
    pub time_created: String,
    #[serde(rename = "EventId")]
    pub event_id: u32,
    #[serde(rename = "Level")]
    pub level: String,
    #[serde(rename = "Provider")]
    pub provider: String,
    #[serde(rename = "Channel")]
    pub channel: String,
    #[serde(rename = "ProcessId")]
    pub process_id: String,
    #[serde(rename = "ThreadId")]
    pub thread_id: String,
    #[serde(rename = "Computer")]
    pub computer: String,
    #[serde(rename = "ChunkNumber")]
    pub chunk_number: i64,
    #[serde(rename = "UserId")]
    pub user_id: String,
    #[serde(rename = "MapDescription")]
    pub map_description: String,
    #[serde(rename = "UserName")]
    pub user_name: String,
    #[serde(rename = "RemoteHost")]
    pub remote_host: String,
    #[serde(rename = "PayloadData1")]
    pub payload_data1: String,
    #[serde(rename = "PayloadData2")]
    pub payload_data2: String,
    #[serde(rename = "PayloadData3")]
    pub payload_data3: String,
    #[serde(rename = "PayloadData4")]
    pub payload_data4: String,
    #[serde(rename = "PayloadData5")]
    pub payload_data5: String,
    #[serde(rename = "PayloadData6")]
    pub payload_data6: String,
    #[serde(rename = "ExecutableInfo")]
    pub executable_info: String,
    #[serde(rename = "HiddenRecord")]
    pub hidden_record: String,
    #[serde(rename = "SourceFile")]
    pub source_file: PathBuf,
    #[serde(rename = "Keywords")]
    pub keywords: String,
    #[serde(rename = "ExtraDataOffset")]
    pub extra_data_offset: i64,
    #[serde(rename = "Payload")]
    pub payload: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EventRecord {
        EventRecord {
            record_number: 42,
            event_record_id: 42,
            time_created: "2026-01-01T00:00:00.0000000Z".into(),
            event_id: 4624,
            level: "LogAlways".into(),
            provider: "Microsoft-Windows-Security-Auditing".into(),
            channel: "Security".into(),
            process_id: "796".into(),
            thread_id: "9456".into(),
            computer: "HOST01".into(),
            chunk_number: 0,
            user_id: String::new(),
            map_description: "An account was successfully logged on".into(),
            user_name: "-\\-".into(),
            remote_host: "- (fe80::1)".into(),
            payload_data1: "SYSTEM".into(),
            payload_data2: String::new(),
            payload_data3: String::new(),
            payload_data4: String::new(),
            payload_data5: String::new(),
            payload_data6: String::new(),
            executable_info: "-".into(),
            hidden_record: "False".into(),
            source_file: std::path::PathBuf::from("Security.evtx"),
            keywords: "Audit success".into(),
            extra_data_offset: 0,
            payload: r#"{"EventData":{"Data":[]}}"#.into(),
        }
    }

    /// The CSV header must match EvtxECmd's 27 columns in this exact order.
    #[test]
    fn csv_header_matches_evtxecmd_order() {
        let mut buf = Vec::new();
        {
            let mut w = csv::Writer::from_writer(&mut buf);
            w.serialize(sample()).unwrap();
            w.flush().unwrap();
        }
        let csv = String::from_utf8(buf).unwrap();
        let header = csv.lines().next().unwrap();
        assert_eq!(
            header,
            "RecordNumber,EventRecordId,TimeCreated,EventId,Level,Provider,Channel,ProcessId,ThreadId,Computer,ChunkNumber,UserId,MapDescription,UserName,RemoteHost,PayloadData1,PayloadData2,PayloadData3,PayloadData4,PayloadData5,PayloadData6,ExecutableInfo,HiddenRecord,SourceFile,Keywords,ExtraDataOffset,Payload"
        );
    }

    #[test]
    fn json_uses_renamed_fields() {
        let json = serde_json::to_string(&sample()).unwrap();
        assert!(json.contains("\"EventId\""));
        assert!(json.contains("\"ChunkNumber\""));
        assert!(!json.contains("event_id"));
    }
}
