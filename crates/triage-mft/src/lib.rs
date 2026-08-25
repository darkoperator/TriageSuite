//! NTFS $MFT / $J (UsnJrnl) / $Boot parser, vendored from the standalone MFTriage
//! project. Records derive `Serialize` (PascalCase) ready for the TriageSuite
//! OutputRouter. CLI/discovery/output live in the `mft-triage` binary crate.

pub mod boot;
pub mod detect;
pub mod error;
pub mod mft;
pub mod usn;
