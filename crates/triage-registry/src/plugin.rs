//! The Rust port of RECmd's IRegistryPluginGrid contract. A plugin matches one
//! or more key paths (with `*` wildcards), processes a key's values, and yields
//! batch rows (ValueType "(plugin)") plus detail-grid rows for a per-plugin CSV.

use notatin::cell_key_node::CellKeyNode;

/// One row a plugin contributes to the BatchCsvOut (ValueData1/2/3) and to its
/// detail CSV (`detail_columns`, ordered to match the RECmd plugin grid).
#[derive(Debug, Clone)]
pub struct PluginRow {
    pub batch_value_name: String,
    pub batch_value_data1: String,
    pub batch_value_data2: String,
    pub batch_value_data3: String,
    /// (column_name, value) pairs for the per-plugin detail CSV, in order.
    pub detail_columns: Vec<(String, String)>,
}

/// A ported RECmd registry plugin.
pub trait RegistryPlugin {
    /// Stable plugin name (RECmd PluginName); used for the detail-CSV basename.
    fn plugin_name(&self) -> &'static str;

    /// Key paths this plugin claims (lowercased compare, `*` wildcards), as in
    /// IRegistryPluginGrid.KeyPaths.
    fn key_paths(&self) -> &[&'static str];

    /// Optional ValueName trigger (IRegistryPluginGrid.ValueName); `None` = key-only.
    fn value_name(&self) -> Option<&'static str> {
        None
    }

    /// Process one matched key's values into plugin rows. `values` yields
    /// (name, raw_bytes, data_type) tuples already extracted from the key, so
    /// the plugin needs no parser handle.
    ///
    /// Plugins that only need the key's own values (e.g. BamDam) implement this
    /// method. Plugins that need to walk subkeys should override
    /// `process_with_hive` instead and leave this as the default no-op.
    fn process(&self, _key: &CellKeyNode, _values: &[PluginValue]) -> Vec<PluginRow> {
        Vec::new()
    }

    /// Process one matched key with access to the hive (for subkey iteration).
    ///
    /// The default implementation delegates to `process()`. Plugins that need
    /// subkeys (AppPaths, UnInstall, ProfileList, Products) override this method
    /// and ignore the `process()` default.
    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        values: &[PluginValue],
        _hive: &mut crate::hive::Hive,
    ) -> Vec<PluginRow> {
        self.process(key, values)
    }
}

/// A value handed to a plugin: pretty name, raw bytes, decoded string, and notatin data type.
///
/// `raw` holds the C# `ValueDataRaw` equivalent:
///   - REG_SZ / REG_EXPAND_SZ: UTF-16LE encoding of the string with a 2-byte null terminator.
///   - Binary / DWORD / QWORD: the literal on-disk bytes (little-endian for integers).
///   - All other types: empty.
///
/// `value_data` holds the human-readable rendered string (C# `KeyValue.ValueData` equivalent),
/// identical to what `triage_registry::value::render_value_data` returns for the decoded
/// `CellValue`.  For most plugins this is all they need — no hive re-read required.
#[derive(Debug, Clone)]
pub struct PluginValue {
    pub name: String,
    pub raw: Vec<u8>,
    pub value_data: String,
    pub data_type: notatin::cell_key_value::CellKeyValueDataTypes,
}
