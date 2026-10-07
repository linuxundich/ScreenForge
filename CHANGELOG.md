# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.34.1] - 2026-10-07

### Fixed

- The classic waves background had visible corners on large canvases: each
  wave was a polygon of 24 points. It is a smooth curve now. Scenes with
  classic waves look slightly softer at the wave edges; nothing else moves.

## [0.34.0] - 2026-10-06

### Added

- Scenes: every composition is kept in the app's scene library with all
  its settings, a preview picture and its last change. The scene overview
  is the start screen: a grid of preview cards, sortable by last change or
  name, searchable, with a selection mode for several scenes at once.
- Scenes save themselves a second after each change and when the window
  closes; Ctrl+S saves right away.
- Card menu and editor title menu: open, rename, duplicate, apply a
  scene's look to the open scene, export as `.screenforge` file, delete
  with undo.
- Import `.screenforge` files as scenes (Ctrl+I, or open them from the
  file manager).

### Changed

- "Save" and "Save As" are gone from the main menu; "Export as File…"
  (Shift+Ctrl+S) writes a scene out to share it. The window title is the
  scene's name.

### Fixed

- Closing the window no longer loses an unsaved composition.

## [0.33.0] - 2026-10-06

### Added

- Three label types to start from: Capsule (dark translucent pill on the
  screenshot), Caption (light chip below the screenshot) and Headline
  (large text above the screenshot, as on app store images). Everything
  stays adjustable afterward.
- Automatic size for labels and callouts: text, padding, radius, edge
  distance and shadow grow and shrink with the screenshot's width, with a
  size percentage for fine-tuning. On by default; switching it off
  freezes the size of a typical phone screenshot.
- Labels can sit above or below the screenshot; the canvas grows to fit.
- Callout looks: Light, Dark and Accent.

### Changed

- Callouts got a new design: pill bubble with a soft shadow, a gently
  curved line with a light halo, and a ringed target dot instead of an
  arrowhead.
- New projects start with the Capsule label; an untouched old default
  label style is replaced by it.

### Fixed

- Dragging callouts, labels and screenshots was slow, worst with tilted
  screenshots (around 300 ms per frame). The preview now keeps every
  screenshot, including the perspective warp, as a ready bitmap at
  preview resolution, which brings a redraw down to roughly 10–20 ms.
  Exports still draw at full sharpness.

## [0.32.1] - 2026-10-06

### Changed

- New app icon: three Android phones, fanned out like the app's fan
  layout, standing in a forge fire that also burns in their screens. Drawn
  to the GNOME app icon guidelines with the GNOME palette, with a matching
  symbolic icon. Replaces the phone on an anvil.

## [0.32.0] - 2026-10-06

### Added

- Redactions on the canvas: the selected screenshot's areas show a dashed
  outline with a handle. Drag an area to move it, drag the handle to
  resize it; both can be undone, Escape cancels the drag.

### Changed

- New icon for "Draw on Screenshot".

## [0.31.0] - 2026-10-06

### Added

- Device and perspective: a "For All Screenshots" switch. Turned off,
  frame, frame color, turn and lean apply to the selected screenshots
  only, and the rows show the selected screenshot's values.
- Redactions: "Draw on Screenshot" — drag a rectangle over the area that
  should disappear instead of typing percent values.
- Watermark: an optional logo image, drawn before the text or on its own.
  Saved into `.screenforge` project files like the screenshots.
- Export: "Export App Store Set…" writes the composition once each for
  iPhone 6.9" (1320 × 2868), iPad 13" (2064 × 2752) and Google Play
  (1080 × 1920) into a folder, split into several images if set.
- Animated WebM export: choice of animation (rise, fade, slide, zoom) and
  tempo (slow, normal, fast).

## [0.30.0] - 2026-10-06

### Added

- Device frames: a generic phone (punch-hole camera), tablet or browser
  window drawn around every screenshot, dark or light. Vector-drawn, so
  there are no manufacturer images to maintain; the layout reserves the
  frame's size, so screenshots keep their exact proportions.
- Perspective tilt: turn the screenshots around their vertical axis and
  lean them around the horizontal one, or fan them out so the outer
  screenshots turn toward the middle. A real perspective warp with a
  shadow cast from the tilted shape.
- Panoramas for app stores: "Split Into" exports the composition as
  several images side by side (`name-1.png`, `name-2.png` …, or one PDF
  page each) with one continuous background; dashed guides on the canvas
  show the cuts. New format presets for the App Store (iPhone 6.9″,
  iPad 13″) set the size of each image.
- Animated WebM export: the screenshots fly in one after another over
  the background (VP9 via GStreamer, about three seconds).
- Command line: `screenforge --preset NAME [--output FILE] [--width N]
  IMAGE…` renders images with a saved preset without opening a window;
  `--list-presets` lists them.
- The presets list shows a preview of each preset's look.

### Changed

- Presets also save the device frame and the watermark.
- Newly added screenshots take the shadow, corner radius, frame and tilt
  of the existing ones, matching what the style controls show.

## [0.29.0] - 2026-10-06

### Added

- Translations via gettext. The source strings are now English, with a
  complete German translation in `po/de.po`; the app follows the system
  language. `po/update-pot.sh` refreshes the catalogs.
- meson build: installs the binary, desktop file, AppStream metainfo,
  GSettings schema, icons, MIME type for `.screenforge` projects and the
  translations, and validates desktop file, metainfo and schema in
  `meson test`.
- Flatpak manifest for the GNOME 51 runtime with an offline build script
  (`build-aux/flatpak/build.sh`). Inside the sandbox, Android import uses
  the host's `adb` through `flatpak-spawn --host`.
- README rewritten as a product page with new screenshots; build and
  project notes moved to `docs/DEVELOPMENT.md`.

### Changed

- An installed build uses the system GSettings schema; only `cargo run`
  still points GLib at the schema compiled into `OUT_DIR`.
- The watermark casts a soft shadow instead of a hard outline.
- Background-only variant previews no longer convert every screenshot,
  which makes the variant grid noticeably faster.
- The in-app release notes are in English.

## [0.28.0] - 2026-10-06

### Added

- Format presets in the export tab: fit to content, 16:9, 1:1, 4:5,
  9:16, Open Graph 1200 × 630, Mastodon/Bluesky 1600 × 900 and the Play
  Store feature graphic 1024 × 500. A fixed format grows the canvas
  around the content, which stays centered; nothing is cropped.
- Copy the finished image to the clipboard (Ctrl+Shift+C, the export tab
  or the floating toolbar), and drag it out of the window via the
  floating toolbar's drag button into a browser, file manager or chat.
- Redactions ("Schwärzen" in the style tab): hide areas of the selected
  screenshot as a dark bar, pixel blocks or a blur, positioned in percent
  of the screenshot and attached to its content.
- Background "Screenshot (unscharf)": the first visible screenshot,
  scaled to fill the canvas and blurred, with adjustable blur and
  brightness.
- "Gleichmäßig verteilen" in the free layout: one row with equal gaps and
  the screenshots' centers on one line.
- Transparent background on export (PNG, WebP, AVIF, PDF; JPEG is
  flattened onto white).
- Text watermark in a chosen corner, with size, opacity and color.
- PDF export (one page, the composition embedded as an image).
- Clean Android status bar: before each adb capture the device is put
  into System UI demo mode (12:00, full battery and signal, no
  notification icons) and back afterwards. On by default, switchable in
  the preferences.
- Desktop screenshots via the screenshot portal ("+" menu), imported
  straight into the composition.
- "Open with": the app accepts image files and `.screenforge` projects
  as arguments; the desktop file declares the image MIME types.

### Fixed

- A panel's initial sync could hit a "RefCell already borrowed" panic if
  it set widgets while holding the editor state; the new panels never do.

## [0.27.1] - 2026-10-06

### Changed

- The window's state is now a GObject (`EditorModel`) that the UI binds
  to: undo/redo availability, title and subtitle, the empty state, and
  which background, generator and alignment rows are visible all follow
  its properties through property bindings and GTK expressions. This
  replaces two duplicated blocks of hand-written visibility code.
- The preset name prompt uses an `AdwEntryRow` and focuses it right away.

### Fixed

- Undo and redo showed as available in a fresh, unedited project (and
  after any focus change), because the text-field guard re-enabled them
  without checking the history. Their state now combines both.

## [0.27.0] - 2026-10-06

### Added

- Background variants: a grid of six alternatives in the background tab,
  one click applies one. "Neue Varianten" rolls a fresh set, "Stile
  mischen" draws them from all styles. The thumbnails show the background
  alone, so the variants stay distinguishable at that size.
- Background studio ("Mehr Varianten …"): a dialog with nine large
  previews of the real composition, a style list (or all styles mixed),
  the mood, and a switch to show or hide the screenshots.
- Empty state: a fresh window shows how to add screenshots instead of an
  empty canvas; drag and drop works on it too.
- Floating canvas toolbar with zoom out, zoom level (opens the zoom
  menu), zoom in and "Screenshots ausblenden".
- Shortcuts dialog (Ctrl+?, also in the main menu) and F9 to toggle the
  sidebar.
- Spinner in the export button while an export runs.
- Header bar title shows the project name, the number of screenshots and
  the export size.

### Changed

- The sidebar is now a set of tabs (Layout, Hintergrund, Stil, Text,
  Export) on the right side of the window instead of one long scrolling
  page.
- Header bar reorganized: an "Add" menu (open images, paste, open
  project) replaces the open menu; zoom and "hide screenshots" moved to
  the floating canvas toolbar; save and save-as are now in the main menu.
- The layout type is a toggle group with icons; the alignment buttons are
  linked icon buttons.
- On windows narrower than 760 px the sidebar becomes an overlay and
  starts hidden.
- Zooming in or out from "fit to window" continues from the current fit
  scale instead of jumping from 100 %.
- `main.rs` is split into modules per topic and per sidebar panel.

## [0.26.0] - 2026-10-06

### Added

- Six new background generator styles: Layers (stacked paper waves),
  Arcs (concentric rings from a corner), Ribbons (wide silk bands),
  Planes (folded paper with diagonal edges), Lines (a bundle of fine
  wave lines) and Mist (soft color fields). All of them draw smooth
  Bézier curves, soft shadows from one light direction and gradients
  inside every shape. Layers is the new default.
- A "Stimmung" (mood) setting for generated palettes: vivid, light or
  dark. Palettes are built in OKLCH as a dark-to-light ramp with a
  complementary accent, so they stay harmonious instead of jumping to
  fully saturated colors. Dark yellow hues, which read as olive, are
  steered away from.
- A "Körnung" (grain) setting: fine film grain over the generated
  background against color banding.
- Core API for deterministic background variants (`generator::variant`),
  the basis for the upcoming variant picker.

### Changed

- The original wave generator is kept as the style "Wellen (klassisch)".
  Projects saved with earlier versions load with that style and without
  grain, so they look exactly as before.

## [0.25.0] - 2026-09-11

### Added

- A callout's target dot now gets a white border whose width always
  matches the arrow's own "Pfeilbreite" (line width), instead of being a
  plain filled circle — keeps the dot legible against a same-colored
  background, and needs no separate setting since it's derived from one
  that already exists.

### Fixed

- Picking a shadow preset ("Kein Schatten"/"Subtil"/…) for a callout's
  own shadow could crash the app with a "RefCell already borrowed"
  panic: the handler held the document open while updating the
  distance/blur sliders, which reentrantly tried to open it again. The
  screenshot-level shadow controls already guarded against exactly this;
  the callout version had the same gap.
- Increasing a callout's "Punktgröße" (dot size) visibly did nothing:
  the on-canvas handle for dragging the callout's target point painted
  an opaque white circle of a fixed size directly over the actual
  rendered dot on every redraw, hiding any size change underneath. The
  handle is now an outline only, so the real dot — and its new border,
  above — stays visible through it.

## [0.24.0] - 2026-09-04

### Added

- Import screenshots from a connected Android device: the toolbar's
  import button now shows live whether a capture is actually possible —
  disabled with an explanatory tooltip when no device is connected, when
  a device is plugged in but "USB-Debugging zulassen?" hasn't been
  confirmed on it yet, or when `adb` itself isn't available — instead of
  only ever failing after the fact when clicked.
- Presets: save the current layout, background, shadow, corner radius,
  and label look under a name from the header bar's new "Presets" menu,
  and reapply, rename, or delete a saved preset later. Replaces the old
  file-based `.screenforge-template` export/import — presets now live in
  the app's own settings instead of a file the user had to manage.
- A collapsible sidebar: toggle the settings panel from the header bar
  to give the canvas more room, e.g. on a narrower window.
- Callouts get an adjustable "Punktgröße" (dot radius): a filled marker
  at the exact point a callout points to, on top of the arrow, sized
  independently so the target is unambiguous even with a thin arrow.
- Independent horizontal/vertical outer margins ("Rand" split into two
  values) for a composition's layout, instead of one uniform margin on
  all four sides.

### Changed

- A screenshot's label can no longer be styled individually — position,
  font, colors, background, padding, corner radius, and shadow are now
  one shared look for every label in the project, editable once and
  applied everywhere; only each label's own text stays per-screenshot.
  A preset now captures this shared look too, and applying one never
  touches any label's text. Existing projects with per-label styling
  still load, with each label's saved style discarded in favor of the
  project's own shared default.
- The generator's `Zufällig` (random) color strategy is now the default
  for a newly generated background, and both it and the manual palette's
  starting colors produce four genuinely distinct hues instead of colors
  that could read as near-identical shades of the same one.
- Saved `.screenforge` project files are now self-contained zip archives
  that embed each screenshot's and background image's own original file
  bytes, so a saved project keeps working even if the source images are
  later moved, renamed, or deleted. Projects saved by older versions
  still load unchanged.

## [0.22.0] - 2026-09-03

### Added

- Callouts ("Feature-Hinweise"): add any number of "look, feature X" text
  bubbles with an arrow to a screenshot, each editable inline in the
  sidebar — text, background/text color, corner radius, arrow color and
  width — with no dialog. Drag the bubble and its arrow's target
  independently on the canvas; the target is stored relative to the
  screenshot, so it keeps pointing at the same spot if the screenshot is
  later resized.

### Changed

- Replaced the single composition-wide "Titel" with a "Label" per
  screenshot: select a screenshot to show and edit its own label
  directly in the sidebar (no dialog), with independent text,
  background (solid, gradient, or a contrast-aware "Automatisch"
  suggestion derived from that screenshot's own colors — never by
  raising saturation), font, text color/alignment, corner radius,
  separate horizontal/vertical padding, and an optional shadow. Drag the
  label freely on the canvas in any layout mode; its position is stored
  relative to its own screenshot, so it stays put through moves,
  resizes and layout changes.
- A newly imported screenshot's default label now scales its font size
  and padding to the screenshot's own width, and starts with a solid
  white background, instead of a fixed small size with no background
  that could look mismatched across very different screenshot sizes.

### Removed

- The composition-wide "Titel" feature (`Document.title`) — replaced by
  the per-screenshot Label above. Old project files with a saved title
  still load; the title text itself doesn't carry over.

## [0.21.0] - 2026-09-03

### Changed

- Reworked the generated background's rendering to match the look of a
  hand-drawn "wave-layer" reference wallpaper: adjacent regions now meet
  at clear, hard edges instead of a soft gradient blur, get their color
  contrast from separating Oklab lightness/hue (never from raising
  saturation), and cast a directionally-consistent contact shadow onto
  the layer behind them for a sense of depth. `Kontrast`/`Weichheit` now
  double as color-separation strength and shadow strength/blur, rather
  than only feeding a soft fade.

### Fixed

- Generated backgrounds with strong contrast/shadow settings no longer
  take long enough to look like the app had hung, and no longer show
  stray straight horizontal/vertical line artifacts — both were caused
  by the same shadow-bitmap sizing bug (the pattern's far-away focus
  point was included in the shadow's own bounding box, and the render-
  resolution cap didn't account for the blur's own padding cost).

## [0.20.0] - 2026-09-03

### Added

- Import a screenshot directly from a connected Android device: "Von
  Android-Gerät importieren…" in the Öffnen menu (`Strg+Umschalt+A`)
  shells out to the user's own `adb` — `adb devices -l` to find the one
  authorized device, `adb exec-out screencap -p` to stream its current
  screen straight to a PNG, no push/pull through the device's own storage
  needed. Works identically over USB or wireless debugging, since that's
  negotiated by `adb` itself. Runs on a background thread and feeds into
  the same import path as a file-picked screenshot, so it's undoable like
  every other import.

## [0.19.0] - 2026-09-03

### Added

- An alignment tool for `LayoutMode::Free`: a new "Ausrichtung" section in
  the sidebar (visible only in free layout) with six buttons — Links/
  Mitte/Rechts and Oben/Mitte/Unten — that align the current
  multi-selection's screenshots against the selection's own bounding box,
  as one undoable step. Requires at least two selected screenshots; a
  toast explains the no-op otherwise, since selection lives entirely in
  the canvas widget and can go stale between clicks.
- An eyedropper ("Farbpipette") next to every color target that makes
  sense to sample from an imported screenshot — background color 1,
  gradient color 2, and all four manual generator colors. Clicking it
  arms a crosshair cursor; the next click on the canvas reads that
  pixel's actual rendered color straight out of the composited surface
  and applies it to the target exactly as if picked from its own color
  dialog, live preview included. Implemented as one temporary,
  capture-phase click gesture added to the canvas and removed the moment
  it fires, so it never touches the canvas's own selection/drag/resize
  handling.

## [0.18.0] - 2026-09-02

### Changed

- Replaced the "Vektor-Muster" background and the old six-style
  ("Flow"/"Waves"/"Blobs"/"Geometry"/"Rays"/"Minimal") generator with a
  single "wave-layer fan" algorithm: nested, wave-perturbed arc layers
  around one focus point, continuously morphing between flat wave bands
  and nested corner arcs via one "Fokus" slider — replicating the
  minimal vector-wallpaper look of the reference SVGs that prompted this
  instead of the previous grab-bag of styles.
- Reduced generated-background color strategies from seven to four:
  Manuell (4 fixed color fields), Von Screenshots (dominant color, with
  "Invertierter Kontrast" sliding between matching and complementary),
  Schwarz-Weiß, and Zufällig.
- The generator dialog no longer exposes Dichte, Fluss, Abwechslung or
  Weichheit as sliders — every "Generieren" click now draws fresh random
  values for them instead (still fully deterministic/reproducible from
  the stored seed on save/load). Kontrast remains manually adjustable.
- A new document now defaults to a linear-gradient background instead of
  a flat color.

### Added

- "Position X"/"Position Y" and "Skalierung" controls for generated
  backgrounds, moving and resizing the wave-layer pattern independently
  of its focus/geometry.
- A "Screenshots ausblenden" toggle in the header bar to temporarily
  preview a background without the screenshots drawn on top — a
  render-only preference that never touches the document, undo history,
  or the saved project.

### Removed

- The "Vektor-Muster" background type (dot grid, diagonal stripes, and
  the freeform circle/line editor) — superseded by the new generator.

### Fixed

- Generated backgrounds no longer show a stray straight diagonal edge
  cutting across the pattern. Each wave layer is a "pie slice" with two
  dead-straight radial sides; their angular sweep is now sized from the
  actual canvas geometry (rather than a fixed formula), so both sides
  always fall outside the visible canvas regardless of focus point,
  seed, or aspect ratio.

### Performance

- Rendered shadow bitmaps are now cached and reused across repaints
  (`core::shadow_cache`) — moving an element no longer re-blurs its
  shadow on every frame.

## [0.17.0] - 2026-09-02

### Added

- Freeform vector shapes: the "Vektor-Muster" background gets a third
  "Benutzerdefiniert" pattern alongside the Dots/DiagonalLines presets —
  add individual circles or lines ("+ Kreis"/"+ Linie"), each with its
  own position, size and color, editable in place and individually
  deletable. Switching into it from an existing Dots/DiagonalLines
  pattern starts from those shapes as an editable starting point, rather
  than discarding them.

## [0.16.0] - 2026-09-02

### Added

- Multi-select: click a screenshot to select it, Shift-click to add or
  remove another, or drag across empty canvas space to marquee-select
  everything it overlaps. Selected screenshots are outlined, and:
  - `Delete`/`Backspace` removes every selected screenshot as one undo
    step (the right-click context menu's "Löschen" still targets only
    whichever screenshot it was opened on, independent of the selection).
  - In `LayoutMode::Free`, dragging any screenshot that's part of a
    multi-selection moves the whole selection together, as one undo
    step.

## [0.15.0] - 2026-09-02

### Added

- Text captions ("Text-Overlay"): an optional single-line or multi-line
  caption drawn over the whole composition, with adjustable position,
  font size and color. Rendered via Cairo's text API directly (no Pango
  dependency), on top of every element, and fully undoable.

## [0.14.0] - 2026-09-02

### Added

- Adjustable shadow direction and length: the "Schatten-Winkel" (0–360°)
  and "Schatten-Distanz" controls replace the fixed offsets baked into
  each shadow preset, exposed as an intuitive angle/distance pair over
  the existing Cartesian offset model.
- Adjustable shadow blur ("Weichzeichner"): a box-blur approximation of
  a Gaussian blur (three passes, horizontal+vertical sliding window),
  applied to the shadow shape before compositing, replacing the
  previously hard-edged shadow.
- Selecting a shadow preset still sets sensible starting values for all
  three controls, but they're now freely adjustable afterwards and
  fully undoable like every other edit.

## [0.13.0] - 2026-09-02

### Added

- Vector-pattern backgrounds ("Vektor-Muster"): a dot grid or diagonal
  stripes, in a chosen color, covering the whole canvas — the first real
  use of the `Background::Decoration` variant that previously only
  existed as a model stub.

## [0.12.0] - 2026-09-02

### Added

- A `GSettings`-backed preferences dialog (`Ctrl+,` or the new primary
  menu button), for the default spacing, margin and export quality used
  for every newly created document. Changing a preference never touches
  the document currently open.

## [0.11.0] - 2026-09-02

### Added

- Reusable templates: save a composition's style (layout mode/spacing/
  margin, background, shadow, corner radius — everything except the
  screenshots themselves) as a `.screenforge-template` file, and load it
  back later to reapply that look to a different set of screenshots.
  Reachable from the "Öffnen" menu ("Vorlage speichern unter…" /
  "Vorlage laden…"). Applying a template is undoable, like every other
  edit.

## [0.10.0] - 2026-09-02

### Fixed

- The canvas now always automatically resizes to fit its content — every
  screenshot plus spacing and margin — instead of staying at a fixed
  default size. Previously, a tall portrait screenshot (e.g. a phone's
  1080×2424 screenshot) could get cropped at the bottom because the
  canvas stayed at its 1920×1080 default regardless of what was imported.

### Changed

- Export size is now a single "Zielbreite" (target width): the
  composition renders scaled so its width matches it, with height always
  following proportionally — never distorted, never cropped. The old
  independent width/height fields are gone; height is shown read-only.

## [0.9.0] - 2026-09-02

### Added

- Snap ("smart") guides while dragging a screenshot in Free mode: edges
  and centers snap into exact alignment with other screenshots and with
  the canvas's own edges/center, with a thin pink guide line drawn for
  each active snap. Pure alignment math lives in `core::snap` and is
  unit-tested independently of the canvas widget.

## [0.8.0] - 2026-09-02

### Added

- Resize handles for Free-mode screenshots: drag any corner to resize,
  keeping the opposite corner anchored. Respects each screenshot's
  aspect-lock (on by default) by deriving height from width. Undoable,
  like every other edit.

### Note

Snap guides and multi-select are still not part of this slice.

## [0.7.0] - 2026-09-02

### Added

- Free layout mode ("Frei"): drag any screenshot anywhere on the canvas to
  position it manually, instead of an automatic arrangement. Switching
  into it snapshots each screenshot's current (auto-computed) placement as
  its starting position, so nothing jumps to the origin. Moves are
  undoable, like every other edit.

### Note

Resizing, snap guides and multi-select are deliberately not part of this
slice — manual positioning ships first, those build on top of it next.

## [0.6.0] - 2026-09-02

### Added

- Image backgrounds (spec §8): pick a file as the composition's
  background, with Cover/Contain/Fill/Tile fitting and an opacity slider,
  alongside solid and gradient backgrounds. Undoable, and rendered
  identically in the live preview and the export.

## [0.5.0] - 2026-09-02

### Added

- AVIF export, alongside PNG, JPEG and WebP, using the quality slider
  already shared with JPEG/WebP.

## [0.4.0] - 2026-09-02

### Added

- Radial gradient backgrounds, alongside solid and linear-gradient (spec
  §8), selectable from the existing "Art" row in the Hintergrund section.
  Centered on the composition; undoable like every other background
  change.

## [0.3.0] - 2026-09-02

### Added

- Vertical and grid layout modes, alongside the existing horizontal one,
  selectable from a new "Art" row in the Layout section of the sidebar
  (spec §4). Vertical stacks screenshots top-to-bottom scaled to a common
  width; grid arranges them into a roughly square, row-scaled grid.
  Switching modes is undoable, like every other edit.

### Fixed

- A `GtkPopoverMenu` used for the per-screenshot context menu could still
  be attached to its parent widget when the window closed, producing a
  harmless but noisy "finalizing widget but it still has children left"
  warning on quit. It's now explicitly unparented on window destroy.

## [0.2.0] - 2026-09-01

### Added

- A right-click context menu on any screenshot (spec §21): Duplicate,
  Replace…, Delete, Bring forward/backward/to front/to back, Rotate 90°,
  and Flip horizontal/vertical.
- Paste a screenshot directly from the clipboard (`Ctrl+V`).
- All of the above are undoable, alongside every existing edit.

## [0.1.0] - 2026-09-01

Initial release: a GTK4 + libadwaita GNOME app for arranging smartphone
screenshots into a single wide presentation image.

### Added

- Import screenshots via a file dialog (`Ctrl+O`) or by dragging them onto
  the canvas; PNG, JPEG and WebP are supported.
- Automatic horizontal layout that scales every screenshot to a common
  height, with adjustable spacing and outer margin.
- Solid-color and linear-gradient backgrounds.
- Shadow presets (None, Subtle, Standard, Strong, Floating) and adjustable
  rounded corners, applied to the whole composition.
- Canvas zoom: fit to window, 100%, step in/out (`Ctrl+0/1/+/-`), with a
  scrollable view once the zoomed content exceeds the visible area.
- Drag-and-drop reordering of screenshots directly on the canvas.
- Undo/redo (`Ctrl+Z` / `Ctrl+Shift+Z`) covering every edit above.
- Export to PNG, JPEG or WebP at a freely configurable resolution,
  rendered on a background thread so the UI never blocks.
- Save and load projects as `.screenforge` files — a versioned JSON format
  that keeps image references as paths rather than copying originals.
- A responsive, HIG-compliant window (header bar, collapsible sidebar,
  toast notifications) built with GTK4 and libadwaita.
