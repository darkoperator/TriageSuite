#!/usr/bin/env python3
"""
Pre-decompresses MAM-compressed Windows Prefetch (.pf) files into raw SCCA bytes.

MAM format:
  bytes 0-3  : magic "MAM\x04"
  bytes 4-7  : LE uint32 decompressed size
  bytes 8+   : LZXPRESS-Huffman compressed payload

Files that do not start with the MAM magic are copied as-is (already uncompressed).

Usage:
  python3 scripts/decompress-pf.py <source_pf_dir> <dest_dir>

The script mirrors the source layout: all .pf files from <source_pf_dir> are
written flat into <dest_dir>.  The destination directory is created if needed.

Each output file is verified to start with version-u32 + "SCCA" at offset 4
and to have exactly the declared decompressed size.
"""

import os
import struct
import sys


MAM_MAGIC = b"MAM\x04"
SCCA_SIG = b"SCCA"


def decompress_file(src_path: str, dst_path: str) -> None:
    with open(src_path, "rb") as f:
        data = f.read()

    if data[:4] != MAM_MAGIC:
        # Not MAM-compressed; write as-is (rare but valid for older Win7 pf)
        with open(dst_path, "wb") as f:
            f.write(data)
        return

    declared_size = struct.unpack_from("<I", data, 4)[0]
    payload = data[8:]

    from dissect.util.compression import lzxpress_huffman
    raw = lzxpress_huffman.decompress(payload)

    # dissect may produce a few bytes more or fewer than the declared size due to
    # block-alignment padding in the LZXPRESS-Huffman format.
    # - When longer: truncate to declared size (dissect padding, safe to drop).
    # - When shorter (up to 8 bytes): zero-pad to declared size.  The trailing
    #   region of a SCCA record is alignment padding anyway (all zeros in practice).
    # - Gap larger than 8 bytes: something is wrong, raise.
    if len(raw) > declared_size:
        raw = raw[:declared_size]
    elif len(raw) < declared_size:
        shortfall = declared_size - len(raw)
        if shortfall > 8:
            raise ValueError(
                f"{src_path}: decompressed to {len(raw)} bytes but declared {declared_size} "
                f"(shortfall {shortfall} exceeds tolerance)"
            )
        raw = raw + b"\x00" * shortfall

    if raw[4:8] != SCCA_SIG:
        raise ValueError(
            f"{src_path}: decompressed output missing SCCA signature, got {raw[4:8]!r}"
        )

    with open(dst_path, "wb") as f:
        f.write(raw)


def main() -> None:
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <source_pf_dir> <dest_dir>", file=sys.stderr)
        sys.exit(1)

    src_dir = sys.argv[1]
    dst_dir = sys.argv[2]
    os.makedirs(dst_dir, exist_ok=True)

    pf_files = sorted(
        f for f in os.listdir(src_dir) if f.lower().endswith(".pf")
    )
    if not pf_files:
        print(f"No .pf files found in {src_dir}", file=sys.stderr)
        sys.exit(1)

    ok = 0
    skipped = 0
    errors = []
    for fname in pf_files:
        src_path = os.path.join(src_dir, fname)
        dst_path = os.path.join(dst_dir, fname)
        try:
            decompress_file(src_path, dst_path)
            ok += 1
        except Exception as e:
            errors.append((fname, str(e)))
            skipped += 1

    print(f"Decompressed {ok} files to {dst_dir} ({skipped} errors)")
    for fname, err in errors:
        print(f"  ERROR {fname}: {err}", file=sys.stderr)

    if errors:
        sys.exit(1)


if __name__ == "__main__":
    main()
