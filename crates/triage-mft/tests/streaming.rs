use triage_mft::usn::{visit_usn_journal, UsnVisitError};

const MIN_RECORD_LENGTH: usize = 60;

fn v2_record(name: &str) -> Vec<u8> {
    let name_bytes = name
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let length = MIN_RECORD_LENGTH + name_bytes.len();
    let mut record = vec![0u8; length];
    record[0..4].copy_from_slice(&(length as u32).to_le_bytes());
    record[4..6].copy_from_slice(&2u16.to_le_bytes());
    record[8..16].copy_from_slice(&42u64.to_le_bytes());
    record[16..24].copy_from_slice(&84u64.to_le_bytes());
    record[56..58].copy_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    record[58..60].copy_from_slice(&(MIN_RECORD_LENGTH as u16).to_le_bytes());
    record[MIN_RECORD_LENGTH..].copy_from_slice(&name_bytes);
    record
}

#[test]
fn visitor_emits_before_reading_the_complete_journal() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("$J");
    let mut bytes = v2_record("first.txt");
    bytes.extend(v2_record("second.txt"));
    bytes.extend(vec![0u8; 8 * 1024 * 1024]);
    std::fs::write(&path, bytes).unwrap();

    let mut seen = Vec::new();
    let result = visit_usn_journal(&path, |record| {
        seen.push(record.name);
        Err("stop after first")
    });

    assert!(matches!(
        result,
        Err(UsnVisitError::Visitor("stop after first"))
    ));
    assert_eq!(seen, ["first.txt"]);
}
