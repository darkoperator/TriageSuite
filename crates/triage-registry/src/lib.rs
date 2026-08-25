//! Reusable Windows Registry engine for RETriage: hive open (with transaction
//! log replay and deleted-record recovery via notatin), hive-type detection,
//! REG_* value rendering matching RECmd, key-tree traversal, and search.

pub mod hive;
pub mod hivetype;
pub mod value;

pub use hive::{Hive, HiveError};
pub use hivetype::HiveType;
pub use value::{
    apply_binary_convert, bytes_to_hex_dashed, raw_bytes, render, render_value_data,
    value_type_string, BinaryConvert, RenderedValue,
};

pub mod search;
pub use search::{search_subtree, HitType, Matcher, SearchHit};

pub mod plugin;
pub use plugin::{PluginRow, PluginValue, RegistryPlugin};
