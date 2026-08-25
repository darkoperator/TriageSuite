#[test]
fn every_capture_lnk_parses_idlist_without_panic() {
    let root = std::path::Path::new("../../test captures");
    if !root.exists() {
        return;
    }
    let mut ok = 0u32;
    let mut with_path = 0u32;
    for p in walk_lnks(root) {
        let raw = std::fs::read(&p).unwrap();
        // skip the 4 $I decoys (not real LNKs); they fail header parse
        let Ok(lnk) = triage_lnk::parse(&raw) else {
            continue;
        };
        ok += 1;
        if !triage_shellitems::absolute_path(&lnk.target_ids).is_empty() {
            with_path += 1;
        }
    }
    assert!(ok > 300, "parsed {ok}");
    assert!(
        with_path * 100 / ok > 70,
        "only {with_path}/{ok} had target paths"
    );
}

fn walk_lnks(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
                    out.push(p);
                }
            }
        }
    }
    out
}
