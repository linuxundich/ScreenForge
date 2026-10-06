# Translating ScreenForge

Source strings are English. Each `.po` file here translates them into one
language; `LINGUAS` lists the languages that get built.

## Updating after UI changes

```sh
po/update-pot.sh
```

This regenerates `screenforge.pot` and merges it into every `.po` file.
Then check what needs work:

```sh
msgattrib --untranslated po/de.po
msgattrib --only-fuzzy po/de.po
```

A `fuzzy` entry is not used at runtime, so read and fix every one rather
than only removing the flag. `msgfmt --check --statistics -o /dev/null
po/de.po` should report no untranslated or fuzzy messages.

`update-pot.sh` uses xgettext for the UI file, the desktop file and the
metainfo, and `extract_rust.py` for the Rust sources: xgettext's own Rust
parser skips strings inside macros such as `glib::clone!`, where much of
the UI text lives.

## In the code

- `gettext("…")` for normal strings, `ngettext("… {count} …", "…", n)` for
  plurals; placeholders are replaced afterwards with `.replace("{count}", …)`.
- `N_("…")` marks strings in `const` tables; translate them with
  `gettext(label)` where they are shown.
- A `// Translators: …` comment right above a call ends up in the `.pot`.

## Adding a language

```sh
msginit -i po/screenforge.pot -o po/xx.po -l xx
echo xx >> po/LINGUAS
```
