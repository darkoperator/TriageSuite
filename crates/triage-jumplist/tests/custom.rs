//! Empirical gate: every `*.customDestinations-ms` file across the captures
//! must parse via the flat binary (header + categories + embedded LNKs +
//! `0xBABFFBAB` footer) path, yielding entries.

#[test]
fn parses_every_custom_destination_in_captures() {
    let root = std::path::Path::new("../../test captures");
    if !root.exists() {
        return;
    }
    let mut ok = 0u32;
    let mut entries = 0u32;
    let mut fail = vec![];
    for p in walk(root, "customdestinations-ms") {
        let raw = std::fs::read(&p).unwrap();
        match triage_jumplist::custom::parse(&raw, 1252) {
            Ok(c) => {
                ok += 1;
                entries += c.entries.len() as u32;
            }
            Err(e) => fail.push(format!("{}: {e}", p.display())),
        }
    }
    assert!(fail.is_empty(), "{ok} ok; failures: {fail:#?}");
    assert!(ok >= 45, "expected 45+ custom dest files, got {ok}");
    assert!(entries > 0, "no custom entries parsed");
}

fn walk(root: &std::path::Path, ext_lower: &str) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().to_lowercase().ends_with(ext_lower))
                {
                    out.push(p);
                }
            }
        }
    }
    out
}
