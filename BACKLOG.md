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

## Phase 2 — GUI overhaul — mostly done in v0.27.0

- [x] Split `main.rs` into modules per topic and sidebar panel (v0.27.0)
- [ ] Editor state as GObject with property bindings instead of
      `syncing_controls`; panel templates of their own
- [ ] `AdwEntryRow` for preset names (still a `GtkEntry` in an alert dialog)
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

## Phase 3 — Quick features

- [ ] Format presets (16:9, 1:1, OG 1200×630, Mastodon, Play feature 1024×500)
- [ ] Copy result to clipboard (Ctrl+Shift+C) and drag-out
- [ ] Clean Android status bar via ADB demo mode (12:00, full battery)
- [ ] Redact/pixelate regions
- [ ] Background from a blurred screenshot
- [ ] Auto-balance (optical centering, equal spacing)
- [ ] Transparent background on export
- [ ] Watermark/logo
- [ ] Desktop screenshot via portal, "Open with" from Nautilus
- [ ] PDF export

## Phase 4 — Translation and packaging

- [ ] English as source language, gettext, German translation in `po/`
- [ ] meson build, Flatpak manifest (GNOME 49), metainfo, proper GSettings
      schema installation instead of the `GSETTINGS_SCHEMA_DIR` workaround
- [ ] README as product page with new screenshots

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
