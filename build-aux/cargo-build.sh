#!/usr/bin/env bash
# Called by meson: builds the app with cargo and copies the binary to the
# path meson expects. Args: SOURCE_ROOT BUILD_ROOT LOCALEDIR BUILDTYPE OUTPUT
set -euo pipefail
source_root="$1"; build_root="$2"; localedir="$3"; buildtype="$4"; output="$5"

export CARGO_TARGET_DIR="$build_root/target"
export SCREENFORGE_LOCALEDIR="$localedir"

if [[ "$buildtype" == "debug" ]]; then
  profile=debug; flags=()
else
  profile=release; flags=(--release)
fi
cargo build "${flags[@]}" --manifest-path "$source_root/Cargo.toml" -p screenforge
cp "$CARGO_TARGET_DIR/$profile/screenforge" "$output"
