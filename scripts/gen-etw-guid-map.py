#!/usr/bin/env python3
"""
Generate crates/re-triage/src/plugins/etw_guid_map.rs from GuidMapping.dll.

Source:  GuidMapping NuGet v2026.5.0
         ~/.nuget/packages/guidmapping/2026.5.0/lib/netstandard2.0/GuidMapping.dll

The DLL contains a single .NET resource key "GuidToName" whose string value
is a sequence of "guid|name" pairs separated by CRLF.  .NET ResourceReader
decodes these as UTF-16 internally (all .NET strings are UTF-16); we receive
them already decoded into Python str objects, so no manual encoding dance is
needed.

Extraction is done by the companion C# tool at
scripts/guid_extractor/Program.cs (built with dotnet build -c Release and
run as shown in the usage below).

Usage (regenerate):
    cd /path/to/TriageSuite
    dotnet run --project scripts/guid_extractor --configuration Release -- \\
        ~/.nuget/packages/guidmapping/2026.5.0/lib/netstandard2.0/GuidMapping.dll \\
        /tmp/guid_pairs_final.txt
    python3 scripts/gen-etw-guid-map.py \\
        /tmp/guid_pairs_final.txt \\
        crates/re-triage/src/plugins/etw_guid_map.rs
"""

import sys
import os
import re

def escape_rust_str(s: str) -> str:
    """Escape a string for use inside a Rust double-quoted string literal."""
    out = []
    for ch in s:
        if ch == '\\':
            out.append('\\\\')
        elif ch == '"':
            out.append('\\"')
        elif ch == '\r':
            out.append('\\r')
        elif ch == '\n':
            out.append('\\n')
        elif ch == '\t':
            out.append('\\t')
        elif ord(ch) < 0x20 or ord(ch) == 0x7f:
            # Other control chars: use \u{XXXX} escape
            out.append(f'\\u{{{ord(ch):04X}}}')
        else:
            # All printable ASCII and all non-ASCII Unicode: emit as-is (UTF-8 source)
            out.append(ch)
    return ''.join(out)

def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <guid_pairs.txt> <output.rs>", file=sys.stderr)
        sys.exit(1)

    pairs_file = sys.argv[1]
    out_file   = sys.argv[2]

    pairs = []
    with open(pairs_file, 'r', encoding='utf-8') as f:
        for lineno, raw_line in enumerate(f, 1):
            line = raw_line.rstrip('\n').rstrip('\r')
            if not line:
                continue
            pipe = line.find('|')
            if pipe < 0:
                print(f"WARNING: no pipe on line {lineno}: {line!r}", file=sys.stderr)
                continue
            guid = line[:pipe]
            name = line[pipe+1:]
            pairs.append((guid, name))

    # Sort by GUID ascending (required for binary_search)
    pairs.sort(key=lambda p: p[0])

    entry_count = len(pairs)
    print(f"Generating {entry_count} entries...", file=sys.stderr)

    lines = []
    lines.append(f'// Auto-generated from GuidMapping.dll ({entry_count:,} entries).')
    lines.append('//')
    lines.append('// Source:  GuidMapping NuGet v2026.5.0')
    lines.append('//')
    lines.append('// Regenerate:')
    lines.append('//   dotnet run --project scripts/guid_extractor --configuration Release -- \\')
    lines.append('//       ~/.nuget/packages/guidmapping/2026.5.0/lib/netstandard2.0/GuidMapping.dll \\')
    lines.append('//       /tmp/guid_pairs_final.txt')
    lines.append('//   python3 scripts/gen-etw-guid-map.py \\')
    lines.append('//       /tmp/guid_pairs_final.txt \\')
    lines.append('//       crates/re-triage/src/plugins/etw_guid_map.rs')
    lines.append('//')
    lines.append('// Encoding: resource string decoded by .NET ResourceReader (UTF-16 internally,')
    lines.append('// emitted here as UTF-8 Rust source). Control characters in names (e.g.')
    lines.append('// \\u{0001} prefix on MEGA entries) are preserved as \\u{XXXX} escapes.')
    lines.append('//')
    lines.append('// The slice MUST remain sorted ascending by GUID for binary_search to work.')
    lines.append('pub static ETW_GUID_MAP: &[(&str, &str)] = &[')

    for guid, name in pairs:
        escaped_name = escape_rust_str(name)
        lines.append(f'    ("{guid}", "{escaped_name}"),')

    lines.append('];')
    lines.append('')
    lines.append('#[cfg(test)]')
    lines.append('mod tests {')
    lines.append('    use super::ETW_GUID_MAP;')
    lines.append('')
    lines.append('    #[test]')
    lines.append('    fn etw_guid_map_is_sorted() {')
    lines.append('        assert!(')
    lines.append('            ETW_GUID_MAP.windows(2).all(|w| w[0].0 <= w[1].0),')
    lines.append('            "ETW_GUID_MAP is not sorted — binary_search will return wrong results"')
    lines.append('        );')
    lines.append('    }')
    lines.append('')
    lines.append('    #[test]')
    lines.append('    fn etw_guid_map_spot_checks() {')
    lines.append('        let lookup = |guid: &str| -> Option<&str> {')
    lines.append('            ETW_GUID_MAP')
    lines.append('                .binary_search_by(|(k, _)| (*k).cmp(guid))')
    lines.append('                .ok()')
    lines.append('                .map(|i| ETW_GUID_MAP[i].1)')
    lines.append('        };')
    lines.append('        // Previously MISSING entry')
    lines.append('        assert_eq!(')
    lines.append('            lookup("cafeefac-0015-0000-0007-abcdeffedcbc"),')
    lines.append('            Some("Java Plug-in 1.5.0_07"),')
    lines.append('        );')
    lines.append('        // Previously TRUNCATED at ® (non-ASCII byte)')
    lines.append('        assert_eq!(')
    lines.append('            lookup("113fae2b-ff38-4054-9229-467410d6eb27"),')
    lines.append('            Some("Intel® AAC encoder"),')
    lines.append('        );')
    lines.append('        // Previously EMPTIED (control char \\u{0001} prefix)')
    lines.append('        assert_eq!(')
    lines.append('            lookup("056d528d-ce28-4194-9ba3-ba2e9197ff8c"),')
    lines.append('            Some("\\u{0001} MEGA (Pending)"),')
    lines.append('        );')
    lines.append('        assert_eq!(')
    lines.append('            lookup("0596c850-7bdd-4c9d-afdf-873be6890637"),')
    lines.append('            Some("\\u{0001} MEGA (Syncing)"),')
    lines.append('        );')
    lines.append('        assert_eq!(')
    lines.append('            lookup("05b38830-f4e9-4329-978b-1dd28605d202"),')
    lines.append('            Some("\\u{0001} MEGA (Synced)"),')
    lines.append('        );')
    lines.append('        // Previously trailing space TRIMMED')
    lines.append('        assert_eq!(')
    lines.append('            lookup("15eae92e-f17a-4431-9f28-805e482dafd4"),')
    lines.append('            Some("Install New Programs "),')
    lines.append('        );')
    lines.append('        // Previously spaces-only EMPTIED')
    lines.append('        assert_eq!(')
    lines.append('            lookup("6da1ed92-315e-4d0b-b354-9d5f519dba95"),')
    lines.append('            Some("   "),')
    lines.append('        );')
    lines.append('    }')
    lines.append('}')
    lines.append('')

    content = '\n'.join(lines)
    with open(out_file, 'w', encoding='utf-8') as f:
        f.write(content)

    print(f"Written: {out_file}", file=sys.stderr)

if __name__ == '__main__':
    main()
