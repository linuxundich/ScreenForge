# Backlog

Overhaul plan decided on 2026-10-06 (analysis, competitor comparison and
mockups: generator prototype with six styles, UI variant "A + B").

## Phase 1 — New background generator (core) — done in v0.26.0

- [x] Style enum: Layers, Arcs, Ribbons, Planes, Lines, Mist, plus the old
      generator kept as "Waves (classic)" so existing projects render unchanged
- [x] OKLCH palette model with moods (vivid, light, dark), "from screenshots"
      hue, complementary accent
- [x] Smooth Bézier curves, soft shadows from one light direction, gradients
      inside every shape, film grain
- [x] Deterministic variants from one seed (same style or mixed styles)
- [x] Minimal controls in the current sidebar (style, mood, grain)

## Phase 2 — GUI overhaul — done in v0.27.0/v0.27.1

- [x] Split `main.rs` into modules per topic and sidebar panel (v0.27.0)
- [x] UI-facing editor state as GObject (`EditorModel`) with property
      bindings and expressions (v0.27.1)
- [x] `AdwEntryRow` for preset names (v0.27.1)
- [ ] Later, if it pays off: move the per-control value sync
      (`syncing_controls`) onto the model too, panel templates of their own
- [x] Sidebar as tabs (`AdwViewStack` + `AdwInlineViewSwitcher`):
      Layout / Background / Style / Text / Export
- [x] New header bar: sidebar toggle, "Add" menu (file, clipboard, Android),
      project title + size, undo/redo, export, main menu (save, shortcuts)
- [x] Floating zoom controls on the canvas, `AdwStatusPage` empty state,
      `AdwSpinner` for export/ADB, `AdwShortcutsDialog`
- [x] `AdwToggleGroup` (layout type), `AdwButtonRow` (generate, variants)
- [x] `AdwBreakpoint`: collapse the sidebar on narrow windows
- [x] Variant grid (6) in the background tab, "More…" opens the background
      studio dialog with large previews of the real composition

## Phase 3 — Quick features — done in v0.28.0

- [x] Format presets (16:9, 1:1, OG 1200×630, Mastodon, Play feature 1024×500)
- [x] Copy result to clipboard (Ctrl+Shift+C) and drag-out
- [x] Clean Android status bar via ADB demo mode (12:00, full battery)
- [x] Redact/pixelate regions
- [x] Background from a blurred screenshot
- [x] Auto-balance (optical centering, equal spacing)
- [x] Transparent background on export
- [x] Watermark/logo
- [x] Desktop screenshot via portal, "Open with" from Nautilus
- [x] PDF export

Follow-ups from phase 3:

- [ ] Redactions: draw and move them directly on the canvas, not only via
      percent values in the sidebar
- [ ] Watermark: optional logo image besides the text

## Phase 4 — Translation and packaging — done in v0.29.0

- [x] English as source language, gettext, German translation in `po/`
- [x] meson build, Flatpak manifest (GNOME 49), metainfo, proper GSettings
      schema installation instead of the `GSETTINGS_SCHEMA_DIR` workaround
- [x] README as product page with new screenshots

Follow-ups from phase 4:

- [ ] Publish a GitHub release with the Flatpak bundle, then submit to Flathub
- [ ] Demo video for the README (see media/app-demo-video)

## Phase 5 — Larger features

- [ ] Generic device frames (phone with punch hole, tablet, browser window)
- [ ] 3D tilt/perspective per screenshot
- [ ] Series/panorama: one background across several exports, App Store
      size sets in one go
- [ ] Preset gallery with thumbnails
- [ ] Command line: `screenforge --preset X images/*.png`
- [ ] Animated export (WebM)

---

Already done from earlier idea lists: alignment tool and eyedropper
(v0.19.0), callouts (v0.22.0).
