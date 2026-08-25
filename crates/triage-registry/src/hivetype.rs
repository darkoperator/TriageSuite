//! Registry hive type: the `.reb` HiveType enum plus detection from a hive's
//! file name. RECmd gates each batch entry on `hive.HiveType == entry.HiveType`
//! (Program.cs ProcessBatch), so both sides must resolve to the same value.

/// Hive types recognized by RECmd's `.reb` batch (ReBatch.cs HiveType_).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiveType {
    Other,
    NtUser,
    Sam,
    Security,
    Software,
    System,
    UsrClass,
    Components,
    Drivers,
    Amcache,
    Syscache,
    Bcd,
    BcdTemplate,
    Elam,
    UserDiff,
    Bbi,
    Vsmidk,
    Default,
    User,
    UserClasses,
    Settings,
    Registry,
}

impl HiveType {
    /// Parse the YAML `HiveType:` string from a `.reb` entry. RECmd matches the
    /// `[Description("...")]` text on the enum (case-insensitive in practice).
    pub fn from_reb(s: &str) -> HiveType {
        match s.trim().to_ascii_uppercase().as_str() {
            "NTUSER" => HiveType::NtUser,
            "SAM" => HiveType::Sam,
            "SECURITY" => HiveType::Security,
            "SOFTWARE" => HiveType::Software,
            "SYSTEM" => HiveType::System,
            "USRCLASS" => HiveType::UsrClass,
            "COMPONENTS" => HiveType::Components,
            "DRIVERS" => HiveType::Drivers,
            "AMCACHE" => HiveType::Amcache,
            "SYSCACHE" => HiveType::Syscache,
            "BCD" => HiveType::Bcd,
            "BCD-TEMPLATE" => HiveType::BcdTemplate,
            "ELAM" => HiveType::Elam,
            "USERDIFF" => HiveType::UserDiff,
            "BBI" => HiveType::Bbi,
            "VSMIDK" => HiveType::Vsmidk,
            "DEFAULT" => HiveType::Default,
            "USER" => HiveType::User,
            "USERCLASSES" => HiveType::UserClasses,
            "SETTINGS" => HiveType::Settings,
            "REGISTRY" => HiveType::Registry,
            _ => HiveType::Other,
        }
    }

    /// Detect the hive type from its file name (RECmd primarily keys off the
    /// hive base name). `UsrClass.dat` → UsrClass; `NTUSER.DAT` → NtUser;
    /// `SOFTWARE`/`SYSTEM`/`SAM`/`SECURITY`/`DEFAULT` by exact stem.
    pub fn from_filename(name: &str) -> HiveType {
        let upper = name.to_ascii_uppercase();
        if upper.starts_with("USRCLASS") {
            HiveType::UsrClass
        } else if upper.starts_with("NTUSER") {
            HiveType::NtUser
        } else if upper == "SOFTWARE" {
            HiveType::Software
        } else if upper == "SYSTEM" {
            HiveType::System
        } else if upper == "SAM" {
            HiveType::Sam
        } else if upper == "SECURITY" {
            HiveType::Security
        } else if upper == "DEFAULT" {
            HiveType::Default
        } else if upper == "AMCACHE.HVE" {
            HiveType::Amcache
        } else {
            HiveType::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reb_strings_map() {
        assert_eq!(HiveType::from_reb("SOFTWARE"), HiveType::Software);
        assert_eq!(HiveType::from_reb("ntuser"), HiveType::NtUser);
        assert_eq!(HiveType::from_reb("BCD-Template"), HiveType::BcdTemplate);
        assert_eq!(HiveType::from_reb("bogus"), HiveType::Other);
    }

    #[test]
    fn filenames_detect() {
        assert_eq!(HiveType::from_filename("NTUSER.DAT"), HiveType::NtUser);
        assert_eq!(HiveType::from_filename("UsrClass.dat"), HiveType::UsrClass);
        assert_eq!(HiveType::from_filename("SOFTWARE"), HiveType::Software);
        assert_eq!(HiveType::from_filename("SYSTEM"), HiveType::System);
        assert_eq!(HiveType::from_filename("foo.bin"), HiveType::Other);
    }
}
