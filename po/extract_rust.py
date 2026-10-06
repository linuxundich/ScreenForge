#!/usr/bin/env python3
"""Extracts gettext("…"), ngettext("…", "…", n) and N_("…") calls from Rust
sources into a .pot file.

xgettext's own Rust parser skips strings inside macros (glib::clone!,
#[error(…)]), which is where much of this app's UI text lives, so this
small regex-based extractor is used for the Rust files instead.
A `// Translators:` comment on the line(s) right above a call is kept.

Usage: extract_rust.py OUTPUT.pot FILE.rs...
"""
import re
import sys

STRING = r'"((?:[^"\\]|\\.)*)"'
CALL = re.compile(r'\b(gettext|N_)\(\s*' + STRING + r'|\bngettext\(\s*' + STRING + r'\s*,\s*' + STRING)


def entries(path):
    text = open(path, encoding="utf-8").read()
    lines = text.split("\n")
    for match in CALL.finditer(text):
        line_no = text.count("\n", 0, match.start()) + 1
        comment = []
        i = line_no - 2
        while i >= 0 and lines[i].strip().startswith("//"):
            stripped = lines[i].strip().lstrip("/").strip()
            comment.insert(0, stripped)
            i -= 1
        comment = [c for c in comment if c.startswith("Translators")] and comment
        if match.group(1):
            yield (match.group(2), None), f"{path}:{line_no}", comment
        else:
            yield (match.group(3), match.group(4)), f"{path}:{line_no}", comment


def main():
    out, files = sys.argv[1], sys.argv[2:]
    seen = {}
    for path in files:
        for key, ref, comment in entries(path):
            entry = seen.setdefault(key, {"refs": [], "comment": []})
            entry["refs"].append(ref)
            if comment and not entry["comment"]:
                entry["comment"] = comment
    with open(out, "w", encoding="utf-8") as f:
        f.write('msgid ""\nmsgstr ""\n"Content-Type: text/plain; charset=UTF-8\\n"\n\n')
        for (singular, plural), entry in seen.items():
            for c in entry["comment"]:
                f.write(f"#. {c}\n")
            f.write("#: " + " ".join(entry["refs"]) + "\n")
            f.write(f'msgid "{singular}"\n')
            if plural is None:
                f.write('msgstr ""\n\n')
            else:
                f.write(f'msgid_plural "{plural}"\nmsgstr[0] ""\nmsgstr[1] ""\n\n')


main()
