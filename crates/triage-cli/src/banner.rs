pub const AUTHOR: &str = "Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>";

/// The banner shown at the start of every run and in --help/--version
/// (spec section 3.1).
pub fn banner(binary_name: &str, version: &str) -> String {
    format!("{binary_name} version {version}\n\nAuthor: {AUTHOR}\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn banner_matches_spec_section_3_1() {
        let b = super::banner("StubTriage", "0.1.0");
        assert_eq!(
            b,
            "StubTriage version 0.1.0\n\nAuthor: Carlos (DarkOperator) Perez <carlos_perez@darkoperator.com>\n"
        );
    }
}
