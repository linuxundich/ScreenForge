#!/usr/bin/env bash
# Regenerates po/screenforge.pot from the sources listed in POTFILES.in and
# merges it into every po/*.po. Run from anywhere; needs gettext and python3.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
mapfile -t files < <(grep -v '^#' po/POTFILES.in | grep -v '^$')
rust=(); other=()
for f in "${files[@]}"; do
  case "$f" in *.rs) rust+=("$f") ;; *) other+=("$f") ;; esac
done
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
python3 po/extract_rust.py "$tmp/rust.pot" "${rust[@]}"
xgettext --from-code=UTF-8 --add-comments=Translators --keyword=_ --keyword=C_:1c,2 \
  -o "$tmp/other.pot" "${other[@]}"
msgcat --use-first "$tmp/other.pot" "$tmp/rust.pot" -o "$tmp/all.pot"
xgettext --from-code=UTF-8 --no-wrap --package-name=screenforge \
  --msgid-bugs-address=https://github.com/linuxundich/ScreenForge/issues \
  -o po/screenforge.pot "$tmp/all.pot"
shopt -s nullglob
for po in po/*.po; do
  msgmerge --quiet --no-wrap --update --backup=none "$po" po/screenforge.pot
done
echo "po/screenforge.pot: $(grep -c '^msgid ' po/screenforge.pot) entries"
