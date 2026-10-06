#!/usr/bin/env bash
# Builds ScreenForge as a Flatpak from the current checkout and installs it
# for the current user.
#
#   build-aux/flatpak/build.sh            # build + install
#   build-aux/flatpak/build.sh --run      # ... and launch it afterwards
#   build-aux/flatpak/build.sh --bundle   # ... and also write screenforge.flatpak
#
# The sandboxed build runs offline, so every crate from Cargo.lock is first
# vendored into .cache/vendor with `cargo vendor` (a quick no-op when
# nothing changed).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
app_id="de.christophlangner.ScreenForge"
manifest="$here/$app_id.json"
cache="$here/.cache"

run=false
bundle=false
for arg in "$@"; do
  case "$arg" in
    --run) run=true ;;
    --bundle) bundle=true ;;
    *) echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done

for tool in flatpak flatpak-builder cargo; do
  command -v "$tool" >/dev/null || { echo "$tool is missing." >&2; exit 1; }
done

# GNOME 51 is based on freedesktop 26.08, which the Rust extension must match.
flatpak install --user --noninteractive --or-update flathub \
  org.gnome.Platform//51 org.gnome.Sdk//51 org.freedesktop.Sdk.Extension.rust-stable//26.08

mkdir -p "$cache"
echo "Vendoring Rust dependencies …"
cargo vendor --locked --quiet --manifest-path "$root/Cargo.toml" "$cache/vendor" >/dev/null
# Only crates.io sources in Cargo.lock, so this fixed source replacement
# is all the offline build needs.
cat > "$cache/cargo-vendor-config.toml" <<'TOML'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "/run/build/screenforge/vendor"
TOML

cd "$here"
flatpak-builder --user --install --force-clean --ccache \
  --state-dir="$here/.flatpak-builder" --repo="$here/repo" \
  "$here/build-dir" "$manifest"

if $bundle; then
  flatpak build-bundle "$here/repo" "$root/screenforge.flatpak" "$app_id"
  echo "Bundle: $root/screenforge.flatpak"
fi

if $run; then
  flatpak run "$app_id"
fi
