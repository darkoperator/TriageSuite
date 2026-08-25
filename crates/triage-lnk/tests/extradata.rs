#[test]
fn extracts_tracker_and_block_list_from_real_lnks() {
    let root = std::path::Path::new("../../test captures");
    if !root.exists() {
        return;
    }
    let mut with_tracker = 0u32;
    let mut with_mac = 0u32;
    let mut with_machine_id = 0u32;
    let mut with_blocks = 0u32;
    let mut total = 0u32;
    for p in walk_lnks(root) {
        let raw = std::fs::read(&p).unwrap();
        let Ok(lnk) = triage_lnk::parse(&raw) else {
            continue;
        };
        total += 1;
        if !lnk.extra_data.blocks_present.is_empty() {
            with_blocks += 1;
        }
        if let Some(t) = &lnk.extra_data.tracker {
            with_tracker += 1;
            // A tracker block always yields a MAC; the machine id is OPTIONAL —
            // a genuinely all-NUL name field decodes to empty, which (matching
            // LECmd's null-nuked JSON) we represent as None. Assert a non-empty
            // name whenever one is present, rather than requiring one.
            if let Some(id) = t.machine_id.as_deref() {
                assert!(
                    !id.is_empty(),
                    "empty machine id should be None in {}",
                    p.display()
                );
                with_machine_id += 1;
            }
            if t.mac_address.is_some() {
                with_mac += 1;
            }
        }
    }
    assert!(total > 300, "parsed {total}");
    assert!(
        with_blocks > 100,
        "expected most LNKs to have >=1 extra block, got {with_blocks}"
    );
    assert!(with_tracker > 0, "no LNK had a tracker block");
    assert!(with_mac > 0, "no LNK tracker yielded a MAC address");
    assert!(with_machine_id > 0, "no LNK tracker yielded a machine id");
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
