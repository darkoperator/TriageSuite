#!/usr/bin/env bash
# Build all TriageSuite forensic tools for 5 platforms and package per-platform
# release archives + checksums. Run from anywhere; resolves the repo root itself.
#
# Usage:
#   scripts/build-release.sh                 # all 5 targets
#   scripts/build-release.sh <triple> ...    # only the named targets
#
# Requires: zig + cargo-zigbuild (brew install zig; cargo install cargo-zigbuild),
# rustup. The host (aarch64-apple-darwin) target builds natively; the rest go
# through cargo-zigbuild (zig cc cross-compiles the bundled SQLite C).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Use the rustup toolchain (where the cross-target std components and the
# cargo-zigbuild subcommand live). A Homebrew/system Rust earlier on PATH would
# otherwise shadow it, and rustup-added targets are invisible to that rustc
# (E0463 "can't find crate for core"). Prepending the cargo bin dir makes the
# rustup shims for cargo/rustc/cargo-zigbuild win.
CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"
[[ -d "$CARGO_BIN" ]] && PATH="$CARGO_BIN:$PATH"

# --- The 12 forensic tool crates -> binary names (StubTriage excluded). ---
PACKAGES=(
  jle-triage le-triage pe-triage rb-triage re-triage
  sbe-triage sqle-triage srume-triage sum-triage wxt-triage
  mft-triage evtx-triage
)
BINARIES=(
  JLETriage LETriage PETriage RBTriage RETriage
  SBETriage SQLETriage SrumETriage SumETriage WxTTriage
  MFTriage EvtxTriage
)

# --- Supported targets. ---
ALL_TARGETS=(
  aarch64-apple-darwin
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-musl
  x86_64-pc-windows-gnu
  aarch64-pc-windows-gnullvm
)

HOST_TARGET="aarch64-apple-darwin"

is_windows() { [[ "$1" == *windows* ]]; }

die() { echo "error: $*" >&2; exit 1; }

# --- Resolve the requested targets. ---
if [[ $# -gt 0 ]]; then
  TARGETS=("$@")
  for t in "${TARGETS[@]}"; do
    found=0
    for a in "${ALL_TARGETS[@]}"; do [[ "$t" == "$a" ]] && found=1; done
    [[ $found -eq 1 ]] || die "unknown target '$t'. valid: ${ALL_TARGETS[*]}"
  done
else
  TARGETS=("${ALL_TARGETS[@]}")
fi

# --- Preflight. ---
command -v rustup >/dev/null 2>&1 || die "rustup not found"
need_zig=0
for t in "${TARGETS[@]}"; do [[ "$t" == "$HOST_TARGET" ]] || need_zig=1; done
if [[ $need_zig -eq 1 ]]; then
  command -v zig >/dev/null 2>&1 || die "zig not found (brew install zig)"
  command -v cargo-zigbuild >/dev/null 2>&1 || die "cargo-zigbuild not found (cargo install cargo-zigbuild)"
fi
echo "==> ensuring rust targets are installed"
for t in "${TARGETS[@]}"; do rustup target add "$t" >/dev/null; done

# --- Version from the workspace Cargo.toml. ---
VERSION="$(awk '/^\[workspace.package\]/{f=1} f&&/^version[[:space:]]*=/{gsub(/[" ]/,"",$3); print $3; exit}' Cargo.toml)"
[[ -n "$VERSION" ]] || die "could not read workspace version from Cargo.toml"
echo "==> TriageSuite v$VERSION"

DIST="$REPO_ROOT/dist"
mkdir -p "$DIST"
PKG_ARGS=(); for p in "${PACKAGES[@]}"; do PKG_ARGS+=(-p "$p"); done
PRODUCED=()

for target in "${TARGETS[@]}"; do
  echo ""
  echo "==> building $target"
  if [[ "$target" == "$HOST_TARGET" ]]; then
    cargo build --release --target "$target" "${PKG_ARGS[@]}" \
      || die "build failed for $target"
  else
    cargo zigbuild --release --target "$target" "${PKG_ARGS[@]}" \
      || die "build failed for $target"
  fi

  name="triagesuite-$VERSION-$target"
  stage="$DIST/$name"
  rm -rf "$stage"; mkdir -p "$stage"

  suffix=""; is_windows "$target" && suffix=".exe"
  for bin in "${BINARIES[@]}"; do
    src="$REPO_ROOT/target/$target/release/$bin$suffix"
    [[ -f "$src" ]] || die "missing built binary: $src"
    cp "$src" "$stage/"
  done

  if is_windows "$target"; then
    archive="$name.zip"
    ( cd "$DIST" && rm -f "$archive" && zip -qr "$archive" "$name" )
  else
    archive="$name.tar.gz"
    tar -czf "$DIST/$archive" -C "$DIST" "$name"
  fi
  rm -rf "$stage"
  PRODUCED+=("$archive")
  echo "    packaged dist/$archive"
done

# --- Checksums over exactly the archives produced this run. ---
echo ""
echo "==> writing dist/SHA256SUMS"
( cd "$DIST" && shasum -a 256 "${PRODUCED[@]}" > SHA256SUMS )

echo ""
echo "==> done. artifacts in dist/:"
for a in "${PRODUCED[@]}"; do
  size="$(du -h "$DIST/$a" | cut -f1)"
  echo "    $a ($size)"
done
echo "    SHA256SUMS"
