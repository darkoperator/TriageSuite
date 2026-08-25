#[test]
fn extracts_paths_and_strings_from_real_lnks() {
    let root = std::path::Path::new("../../test captures");
    if !root.exists() {
        return;
    }
    let mut have_local = 0u32;
    let mut have_args_or_wd = 0u32;
    let mut have_relpath = 0u32;
    let mut total = 0u32;
    for p in walk_lnks(root) {
        let raw = std::fs::read(&p).unwrap();
        let Ok(lnk) = triage_lnk::parse_with_codepage(&raw, 1252) else {
            continue;
        };
        total += 1;
        assert!(
            lnk.extra_data_offset <= raw.len(),
            "extra_data_offset {} > file length {} for {:?}",
            lnk.extra_data_offset,
            raw.len(),
            p
        );
        assert!(
            lnk.extra_data_offset > 0,
            "extra_data_offset is 0 for {p:?}"
        );
        if lnk
            .link_info
            .as_ref()
            .and_then(|l| l.local_path.as_ref())
            .is_some()
        {
            have_local += 1;
        }
        if lnk.string_data.arguments.is_some() || lnk.string_data.working_directory.is_some() {
            have_args_or_wd += 1;
        }
        if lnk.string_data.relative_path.is_some() {
            have_relpath += 1;
        }
    }
    assert!(total > 300, "parsed {total}");
    assert!(have_local > 0, "no LNK had a LinkInfo local path");
    assert!(have_args_or_wd > 0, "no LNK had arguments/working dir");
    assert!(
        have_relpath > 200,
        "expected 200+ relative paths, got {have_relpath}"
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
