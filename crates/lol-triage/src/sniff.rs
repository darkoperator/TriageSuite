#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    AmcacheFileEntries,
    AmcacheDriveBinaries,
    AppCompatCache,
    Mft,
    Prefetch,
    RecycleBin,
}

pub fn sniff(header_line: &str) -> Option<SourceKind> {
    match header_line {
        "ApplicationName,ProgramId,FileKeyLastWriteTimestamp,SHA1,IsOsComponent,FullPath,Name,FileExtension,LinkDate,ProductName,Size,Version,ProductVersion,LongPathHash,BinaryType,IsPeFile,BinFileVersion,BinProductVersion,Usn,Language,Description" => {
            Some(SourceKind::AmcacheFileEntries)
        }
        "KeyName,KeyLastWriteTimestamp,DriverTimeStamp,DriverLastWriteTime,DriverName,DriverInBox,DriverIsKernelMode,DriverSigned,DriverCheckSum,DriverCompany,DriverId,DriverPackageStrongName,DriverType,DriverVersion,ImageSize,Inf,Product,ProductVersion,Service,WdfVersion" => {
            Some(SourceKind::AmcacheDriveBinaries)
        }
        "ControlSet,CacheEntryPosition,Path,LastModifiedTimeUTC,Executed,Duplicate,SourceFile" => {
            Some(SourceKind::AppCompatCache)
        }
        "EntryNumber,SequenceNumber,InUse,ParentEntryNumber,ParentSequenceNumber,ParentPath,FileName,Extension,FileSize,ReferenceCount,ReparseTarget,IsDirectory,HasAds,IsAds,SI<FN,uSecZeros,Copied,SiFlags,NameType,Created0x10,Created0x30,LastModified0x10,LastModified0x30,LastRecordChange0x10,LastRecordChange0x30,LastAccess0x10,LastAccess0x30,UpdateSequenceNumber,LogfileSequenceNumber,SecurityId,ObjectIdFileDroid,LoggedUtilStream,ZoneIdContents,SourceFile" => {
            Some(SourceKind::Mft)
        }
        "Note,SourceFilename,SourceCreated,SourceModified,SourceAccessed,ExecutableName,Hash,Size,Version,RunCount,LastRun,PreviousRun0,PreviousRun1,PreviousRun2,PreviousRun3,PreviousRun4,PreviousRun5,PreviousRun6,Volume0Name,Volume0Serial,Volume0Created,Volume1Name,Volume1Serial,Volume1Created,Directories,FilesLoaded,ParsingError" => {
            Some(SourceKind::Prefetch)
        }
        "SourceName,FileType,FileName,FileSize,DeletedOn" => Some(SourceKind::RecycleBin),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_all_six_known_headers() {
        assert_eq!(
            sniff("ApplicationName,ProgramId,FileKeyLastWriteTimestamp,SHA1,IsOsComponent,FullPath,Name,FileExtension,LinkDate,ProductName,Size,Version,ProductVersion,LongPathHash,BinaryType,IsPeFile,BinFileVersion,BinProductVersion,Usn,Language,Description"),
            Some(SourceKind::AmcacheFileEntries)
        );
        assert_eq!(
            sniff("KeyName,KeyLastWriteTimestamp,DriverTimeStamp,DriverLastWriteTime,DriverName,DriverInBox,DriverIsKernelMode,DriverSigned,DriverCheckSum,DriverCompany,DriverId,DriverPackageStrongName,DriverType,DriverVersion,ImageSize,Inf,Product,ProductVersion,Service,WdfVersion"),
            Some(SourceKind::AmcacheDriveBinaries)
        );
        assert_eq!(
            sniff("ControlSet,CacheEntryPosition,Path,LastModifiedTimeUTC,Executed,Duplicate,SourceFile"),
            Some(SourceKind::AppCompatCache)
        );
        assert_eq!(
            sniff("EntryNumber,SequenceNumber,InUse,ParentEntryNumber,ParentSequenceNumber,ParentPath,FileName,Extension,FileSize,ReferenceCount,ReparseTarget,IsDirectory,HasAds,IsAds,SI<FN,uSecZeros,Copied,SiFlags,NameType,Created0x10,Created0x30,LastModified0x10,LastModified0x30,LastRecordChange0x10,LastRecordChange0x30,LastAccess0x10,LastAccess0x30,UpdateSequenceNumber,LogfileSequenceNumber,SecurityId,ObjectIdFileDroid,LoggedUtilStream,ZoneIdContents,SourceFile"),
            Some(SourceKind::Mft)
        );
        assert_eq!(
            sniff("Note,SourceFilename,SourceCreated,SourceModified,SourceAccessed,ExecutableName,Hash,Size,Version,RunCount,LastRun,PreviousRun0,PreviousRun1,PreviousRun2,PreviousRun3,PreviousRun4,PreviousRun5,PreviousRun6,Volume0Name,Volume0Serial,Volume0Created,Volume1Name,Volume1Serial,Volume1Created,Directories,FilesLoaded,ParsingError"),
            Some(SourceKind::Prefetch)
        );
        assert_eq!(
            sniff("SourceName,FileType,FileName,FileSize,DeletedOn"),
            Some(SourceKind::RecycleBin)
        );
    }

    #[test]
    fn unknown_header_returns_none() {
        assert_eq!(sniff("Foo,Bar,Baz"), None);
        assert_eq!(sniff(""), None);
    }
}
