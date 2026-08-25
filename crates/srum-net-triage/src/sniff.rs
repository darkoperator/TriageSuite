//! Header sniffing for SrumETriage's NetworkUsage/NetworkConnection CSV
//! output. Matches `crates/srume-triage/src/datasets.rs` field declaration
//! order (`NetworkUsageRecord`, `NetworkConnectionRecord`) exactly, since
//! SrumNetTriage consumes SrumETriage's already-produced CSV rather than
//! re-parsing SRUDB.dat itself.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    NetworkUsage,
    NetworkConnection,
}

pub fn sniff(header_line: &str) -> Option<SourceKind> {
    match header_line {
        "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,UserId,AppId,BytesReceived,BytesSent,InterfaceLuid,InterfaceType,L2ProfileFlags,L2ProfileId,ProfileName" => {
            Some(SourceKind::NetworkUsage)
        }
        "Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,UserId,AppId,ConnectedTime,ConnectStartTime,InterfaceLuid,InterfaceType,L2ProfileFlags,L2ProfileId,ProfileName" => {
            Some(SourceKind::NetworkConnection)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_network_usage_header() {
        assert_eq!(
            sniff("Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,UserId,AppId,BytesReceived,BytesSent,InterfaceLuid,InterfaceType,L2ProfileFlags,L2ProfileId,ProfileName"),
            Some(SourceKind::NetworkUsage)
        );
    }

    #[test]
    fn recognizes_network_connection_header() {
        assert_eq!(
            sniff("Id,Timestamp,ExeInfo,ExeInfoDescription,ExeTimestamp,SidType,Sid,UserName,UserId,AppId,ConnectedTime,ConnectStartTime,InterfaceLuid,InterfaceType,L2ProfileFlags,L2ProfileId,ProfileName"),
            Some(SourceKind::NetworkConnection)
        );
    }

    #[test]
    fn unknown_header_returns_none() {
        assert_eq!(sniff("Foo,Bar,Baz"), None);
        assert_eq!(sniff(""), None);
    }
}
