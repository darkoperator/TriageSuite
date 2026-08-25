//! Empirical gate: every `*.automaticDestinations-ms` file across the captures
//! must parse via the compound-file + DestList path, yielding entries and
//! embedded LNKs, and `with_dir` must never reduce the entry count.

#[test]
fn parses_every_automatic_destination_in_captures() {
    let root = std::path::Path::new("../../test captures");
    if !root.exists() {
        return;
    }
    let mut ok = 0u32;
    let mut entries = 0u32;
    let mut with_lnk = 0u32;
    let mut fail = vec![];
    for p in walk(root, "automaticdestinations-ms") {
        let raw = std::fs::read(&p).unwrap();
        match triage_jumplist::automatic::parse(&raw, 1252, false) {
            Ok(a) => {
                ok += 1;
                entries += a.entries.len() as u32;
                with_lnk += a.entries.iter().filter(|e| e.lnk.is_some()).count() as u32;
            }
            Err(e) => fail.push(format!("{}: {e}", p.display())),
        }
    }
    assert!(fail.is_empty(), "{ok} ok; failures: {fail:#?}");
    assert!(ok >= 60, "expected 60+ automatic dest files, got {ok}");
    assert!(
        entries > 0 && with_lnk > 0,
        "entries {entries}, with_lnk {with_lnk}"
    );
}

#[test]
fn with_dir_does_not_reduce_entries() {
    let root = std::path::Path::new("../../test captures");
    if !root.exists() {
        return;
    }
    for p in walk(root, "automaticdestinations-ms") {
        let raw = std::fs::read(&p).unwrap();
        if let (Ok(a), Ok(b)) = (
            triage_jumplist::automatic::parse(&raw, 1252, false),
            triage_jumplist::automatic::parse(&raw, 1252, true),
        ) {
            assert!(
                b.entries.len() >= a.entries.len(),
                "withDir reduced entries for {}",
                p.display()
            );
        }
    }
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
