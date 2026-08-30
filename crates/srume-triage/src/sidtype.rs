//! Port of `Srum.cs` `GetSidTypeFromSidString` — pure string logic, no Windows API.
//!
//! Returns the `SidTypeEnum` variant name exactly as SrumECmd's CsvHelper emits
//! it (enum variant `.ToString()` in C#), e.g. `"Administrator"`,
//! `"UnknownOrUserSid"`, `"LocalSystem"`.

/// Resolve a SID string to its SidTypeEnum variant name (SrumECmd-compatible).
/// Empty string or unresolvable SID returns `"UnknownOrUserSid"`.
pub fn sid_type(sid: &str) -> String {
    if sid.is_empty() {
        return "UnknownOrUserSid".to_string();
    }

    // Primary switch — exact matches first (mirrors the C# `switch (sid)` block).
    let from_switch = match sid {
        "S-1-0-0" => Some("Null"),
        "S-1-1-0" => Some("Everyone"),
        "S-1-2-0" => Some("Local"),
        "S-1-2-1" => Some("ConsoleLogon"),
        "S-1-3-0" => Some("CreatorOwner"),
        "S-1-3-1" => Some("CreatorGroup"),
        "S-1-3-2" => Some("OwnerServer"),
        "S-1-3-3" => Some("GroupServer"),
        "S-1-3-4" => Some("OwnerServer"),
        "S-1-5-1" => Some("Dialup"),
        "S-1-5-2" => Some("Network"),
        "S-1-5-3" => Some("Batch"),
        "S-1-5-4" => Some("Interactive"),
        "S-1-5-6" => Some("Service"),
        "S-1-5-7" => Some("Anonymous"),
        "S-1-5-8" => Some("Proxy"),
        "S-1-5-9" => Some("EnterpriseDomainControllers"),
        "S-1-5-10" => Some("PrincipalSelf"),
        "S-1-5-11" => Some("AuthenticatedUsers"),
        "S-1-5-12" => Some("RestrictedCode"),
        "S-1-5-13" => Some("TerminalServerUser"),
        "S-1-5-14" => Some("RemoteInteractiveLogon"),
        "S-1-5-15" => Some("ThisOrganization"),
        "S-1-5-17" => Some("Iusr"),
        "S-1-5-18" => Some("LocalSystem"),
        "S-1-5-19" => Some("LocalService"),
        "S-1-5-20" => Some("NetworkService"),
        "S-1-5-21-0-0-0-496" => Some("CompoundedAuthentication"),
        "S-1-5-21-0-0-0-497" => Some("ClaimsValid"),
        "S-1-5-32-544" => Some("BuiltinAdministrators"),
        "S-1-5-32-545" => Some("BuiltinUsers"),
        "S-1-5-32-546" => Some("BuiltinGuests"),
        "S-1-5-32-547" => Some("PowerUsers"),
        "S-1-5-32-548" => Some("AccountOperators"),
        "S-1-5-32-549" => Some("ServerOperators"),
        "S-1-5-32-550" => Some("PrinterOperators"),
        "S-1-5-32-551" => Some("BackupOperators"),
        "S-1-5-32-552" => Some("Replicator"),
        "S-1-5-32-554" => Some("AliasPrew2Kcompacc"),
        "S-1-5-32-555" => Some("RemoteDesktop"),
        "S-1-5-32-556" => Some("NetworkConfigurationOps"),
        "S-1-5-32-557" => Some("IncomingForestTrustBuilders"),
        "S-1-5-32-558" => Some("PerfmonUsers"),
        "S-1-5-32-559" => Some("PerflogUsers"),
        "S-1-5-32-560" => Some("WindowsAuthorizationAccessGroup"),
        "S-1-5-32-561" => Some("TerminalServerLicenseServers"),
        "S-1-5-32-562" => Some("DistributedComUsers"),
        "S-1-5-32-568" => Some("IisIusrs"),
        "S-1-5-32-569" => Some("CryptographicOperators"),
        "S-1-5-32-573" => Some("EventLogReaders"),
        "S-1-5-32-574" => Some("CertificateServiceDcomAccess"),
        "S-1-5-32-575" => Some("RdsRemoteAccessServers"),
        "S-1-5-32-576" => Some("RdsEndpointServers"),
        "S-1-5-32-577" => Some("RdsManagementServers"),
        "S-1-5-32-578" => Some("HyperVAdmins"),
        "S-1-5-32-579" => Some("AccessControlAssistanceOps"),
        "S-1-5-32-580" => Some("RemoteManagementUsers"),
        "S-1-5-33" => Some("WriteRestrictedCode"),
        "S-1-5-64-10" => Some("NtlmAuthentication"),
        "S-1-5-64-14" => Some("SchannelAuthentication"),
        "S-1-5-64-21" => Some("DigestAuthentication"),
        "S-1-5-65-1" => Some("ThisOrganizationCertificate"),
        "S-1-5-80" => Some("NtService"),
        "S-1-5-84-0-0-0-0-0" => Some("UserModeDrivers"),
        "S-1-5-113" => Some("LocalAccount"),
        "S-1-5-114" => Some("LocalAccountAndMemberOfAdministratorsGroup"),
        "S-1-5-1000" => Some("OtherOrganization"),
        "S-1-15-2-1" => Some("AllAppPackages"),
        "S-1-16-0" => Some("MlUntrusted"),
        "S-1-16-4096" => Some("MlLow"),
        "S-1-16-8192" => Some("MlMedium"),
        "S-1-16-8448" => Some("MlMediumPlus"),
        "S-1-16-12288" => Some("MlHigh"),
        "S-1-16-16384" => Some("MlSystem"),
        "S-1-16-20480" => Some("MlProtectedProcess"),
        "S-1-18-1" => Some("AuthenticationAuthorityAssertedIdentity"),
        "S-1-18-2" => Some("ServiceAssertedIdentity"),
        _ => None,
    };

    if let Some(name) = from_switch {
        return name.to_string();
    }

    // Prefix/suffix checks for the well-known RID/authority patterns. In the C#
    // source these run inside an `if (sidType == SidTypeEnum.UnknownOrUserSid)`
    // guard; we run them unconditionally here. That is a deliberate
    // simplification: no SID that matches an exact switch arm above also matches
    // any prefix/suffix rule below (the only `S-1-5-21-*` switch arms end in
    // `-496`/`-497`, not the suffix RIDs), so the guard is a no-op in practice.
    // The `if` arms themselves are unconditional (no `else if`) so a later match
    // overrides an earlier one — matching the C# arm order.

    let mut resolved: Option<&'static str> = None;

    if sid.starts_with("S-1-5-5-") {
        resolved = Some("LogonId");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-498") {
        resolved = Some("EnterpriseDomainControllers");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-500") {
        resolved = Some("Administrator");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-501") {
        resolved = Some("Guest");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-512") {
        resolved = Some("DomainAdmins");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-513") {
        resolved = Some("DomainUsers");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-514") {
        resolved = Some("DomainGuests");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-515") {
        resolved = Some("DomainComputers");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-516") {
        resolved = Some("DomainDomainControllers");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-517") {
        resolved = Some("CertPublishers");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-518") {
        resolved = Some("SchemaAdministrators");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-519") {
        resolved = Some("EnterpriseAdmins");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-520") {
        resolved = Some("GroupPolicyCreatorOwners");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-521") {
        resolved = Some("ReadonlyDomainControllers");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-522") {
        resolved = Some("CloneableControllers");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-525") {
        resolved = Some("ProtectedUsers");
    }
    if sid.starts_with("S-1-5-21-") && sid.ends_with("-553") {
        resolved = Some("RasServers");
    }

    resolved.unwrap_or("UnknownOrUserSid").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_wellknown_sids() {
        assert_eq!(sid_type("S-1-5-18"), "LocalSystem");
        assert_eq!(sid_type("S-1-5-19"), "LocalService");
        assert_eq!(sid_type("S-1-5-20"), "NetworkService");
    }

    #[test]
    fn administrator_suffix() {
        // A -500 RID is always the built-in Administrator.
        assert_eq!(
            sid_type("S-1-5-21-1111111111-2222222222-3333333333-500"),
            "Administrator"
        );
    }

    #[test]
    fn unknown_user_sid() {
        // An ordinary user RID has no special classification.
        assert_eq!(
            sid_type("S-1-5-21-1111111111-2222222222-3333333333-1313"),
            "UnknownOrUserSid"
        );
    }

    #[test]
    fn empty_sid_is_unknown() {
        assert_eq!(sid_type(""), "UnknownOrUserSid");
    }

    #[test]
    fn logon_id_prefix() {
        assert_eq!(sid_type("S-1-5-5-0-123456"), "LogonId");
    }

    #[test]
    fn domain_admins_suffix() {
        assert_eq!(sid_type("S-1-5-21-111-222-333-512"), "DomainAdmins");
    }

    #[test]
    fn domain_users_suffix() {
        assert_eq!(sid_type("S-1-5-21-111-222-333-513"), "DomainUsers");
    }
}
