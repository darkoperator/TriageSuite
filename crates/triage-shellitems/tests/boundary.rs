//! Shell items come out of registry values and LNK files byte for byte, so
//! every parser here takes a slice whose length and contents are
//! attacker-controlled. The sweep is over slice shapes rather than integers:
//! truncated, empty and all-ones are what a crafted item looks like, and a
//! parser that indexes past the end of one panics instead of reporting it as
//! undecodable -- turning a single malformed item into a failed parse of the
//! whole artifact.
//!
//! `extension.rs` keeps its own copy of the FILETIME arithmetic rather than
//! depending on `triage-core`, because this crate is deliberately a leaf with
//! three dependencies; that copy is covered here instead.

use triage_shellitems::extension::{
    has_beef0004, has_beef001d, has_beef001e, has_beef0026, parse_beef0004,
    parse_beef001d_executable, parse_beef001e_pintype, parse_beef0026,
};

/// Every length from empty to past the largest fixed offset any of these
/// parsers reads, against fill bytes that make every length field maximal.
fn hostile_bodies() -> Vec<Vec<u8>> {
    let mut bodies = Vec::new();
    for len in 0..96usize {
        for fill in [0x00u8, 0xff, 0x41] {
            bodies.push(vec![fill; len]);
        }
    }
    // A body whose embedded FILETIMEs are the extremes, past a plausible
    // signature position.
    let mut extremes = vec![0u8; 96];
    extremes[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
    extremes[16..24].copy_from_slice(&(1u64 << 63).to_le_bytes());
    bodies.push(extremes);
    bodies
}

#[test]
fn every_extension_block_parser_is_total_over_hostile_bodies() {
    for body in hostile_bodies() {
        let _ = has_beef0004(&body);
        let _ = parse_beef0004(&body);
        let _ = has_beef001d(&body);
        let _ = has_beef001e(&body);
        let _ = parse_beef001d_executable(&body);
        let _ = parse_beef001e_pintype(&body);
        let _ = has_beef0026(&body);
        let _ = parse_beef0026(&body);
    }
}
