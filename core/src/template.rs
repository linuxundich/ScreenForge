//! Reusable style presets: everything that makes a composition *look* the
//! way it does (layout mode/spacing/margin, background, shadow, corner
//! radius, and global label defaults) captured separately from the
//! screenshots/labels themselves, so it can be saved once — under a name
//! the user picks — and reapplied to a different composition later.
//!
//! Deliberately *not* file-based: earlier versions of this saved/loaded a
//! standalone `.screenforge-template` file the user managed themselves.
//! Presets now live inside the app's own persistent settings (the app
//! layer serializes/deserializes the list this module defines to/from a
//! single `GSettings` string key — see `app/src/main.rs`'s
//! `PresetStore`), so this module only defines the data shape and its
//! JSON (de)serialization, with no `Path`/file-I/O of its own.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{Background, CornerRadius, Document, LabelStyle, LayoutSettings, ShadowParams, DeviceFrame, Watermark};

pub const CURRENT_VERSION: u32 = 1;

/// A document's style, independent of which screenshots (or their
/// individual label content) it holds. Shadow and corner radius are
/// captured as a single value rather than per-element, matching the
/// existing assumption elsewhere (spec §27) that these apply uniformly
/// across the whole composition — there's no per-element UI for them yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub layout: LayoutSettings,
    pub background: Background,
    pub shadow: ShadowParams,
    pub corner_radius: CornerRadius,
    /// The shared label look every label in the project uses (spec:
    /// "globale Label-Einstellungen" is one of the things a preset
    /// captures) — never any label's own *content*, which stays exactly
    /// what it was before the preset was applied (see
    /// `crate::command::ApplyTemplate`).
    #[serde(default)]
    pub label_defaults: LabelStyle,
    /// Device frame around every screenshot (v0.30.0; none before).
    #[serde(default)]
    pub frame: DeviceFrame,
    /// Watermark (v0.30.0; none before).
    #[serde(default)]
    pub watermark: Watermark,
}

impl Template {
    /// Captures `doc`'s current style. Shadow/corner radius are read from
    /// the first element (if any) — again mirroring the rest of the app's
    /// "uniform across all elements" assumption for these two properties.
    pub fn from_document(doc: &Document) -> Self {
        let (shadow, corner_radius, frame) = match doc.elements.first() {
            Some(el) => (el.shadow, el.corner_radius, el.frame),
            None => (ShadowParams::default(), CornerRadius::default(), DeviceFrame::default()),
        };
        Template {
            layout: doc.layout,
            background: doc.background.clone(),
            shadow,
            corner_radius,
            label_defaults: doc.label_defaults.clone(),
            frame,
            watermark: doc.watermark.clone(),
        }
    }
}

/// Renders `template` on three placeholder phone screens, `width` pixels
/// wide — the preview image in the preset list. Deterministic, needs no
/// real screenshots.
pub fn render_preview(template: &Template, width: i32) -> Result<cairo::ImageSurface, crate::render::RenderError> {
    let accents = [(0.21, 0.52, 0.89), (0.18, 0.62, 0.45), (0.85, 0.45, 0.20)];
    let (sw, sh) = (360.0, 780.0);
    let mut doc = Document::new();
    let mut images = std::collections::HashMap::new();
    for accent in accents {
        let mut element = crate::model::ScreenshotElement::new(crate::model::ImageSource::Path("preview".into()), sw, sh);
        element.shadow = template.shadow;
        element.corner_radius = template.corner_radius;
        element.frame = template.frame;
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, sw as i32, sh as i32)?;
        {
            let ctx = cairo::Context::new(&surface)?;
            ctx.set_source_rgb(0.97, 0.97, 0.98);
            ctx.paint()?;
            ctx.set_source_rgb(accent.0, accent.1, accent.2);
            ctx.rectangle(0.0, 0.0, sw, sh * 0.16);
            ctx.fill()?;
            ctx.set_source_rgb(0.86, 0.87, 0.89);
            for k in 0..6 {
                let y = sh * (0.24 + k as f64 * 0.11);
                let w = sw * if k % 2 == 0 { 0.8 } else { 0.55 };
                crate::render::rounded_rect_path(&ctx, sw * 0.1, y, w, sh * 0.05, &CornerRadius::uniform(sh * 0.02));
                ctx.fill()?;
            }
        }
        images.insert(element.id, surface);
        doc.elements.push(element);
    }
    doc.layout = template.layout;
    doc.background = template.background.clone();
    doc.watermark = template.watermark.clone();
    crate::layout::fit_canvas_to_content(&mut doc);
    let scale = width as f64 / doc.canvas.export_width.max(1) as f64;
    let height = (doc.canvas.export_height as f64 * scale).round().max(1.0) as i32;
    let target = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    crate::render::compose(
        &doc,
        &target,
        scale,
        &images,
        None,
        &crate::shadow_cache::ShadowCache::new(),
        &crate::background_cache::BackgroundCache::new(),
    )?;
    Ok(target)
}

/// One saved preset together with the name the user gave it (spec: "Der
/// Benutzer kann: neues Preset speichern, Preset auswählen, ... Preset
/// umbenennen, Preset löschen" — `name` is the only thing a rename
/// touches).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedPreset {
    pub name: String,
    pub template: Template,
}

#[derive(Debug, Serialize, Deserialize)]
struct PresetListFile {
    format: String,
    version: u32,
    presets: Vec<NamedPreset>,
}

#[derive(Debug, Error)]
pub enum PresetError {
    #[error("saved presets are damaged or not valid ScreenForge JSON: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("unsupported preset list format version {found} (this version of ScreenForge supports up to {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
}

/// Serializes the full list of saved presets to one JSON string, the shape
/// `app/src/main.rs`'s `PresetStore` writes straight into a `GSettings`
/// string key (`presets`) — see this module's own doc comment for why
/// that's the only persistence this needs to support, unlike the old
/// per-file `Template::save`/`load` it replaces.
pub fn serialize_presets(presets: &[NamedPreset]) -> String {
    let file = PresetListFile { format: "screenforge-presets".to_string(), version: CURRENT_VERSION, presets: presets.to_vec() };
    // Only fails on a type that can't be represented as JSON at all (e.g.
    // a non-string map key) — nothing in `Template`'s own shape can ever
    // trigger that, so this is safe to unwrap rather than thread a
    // `Result` through every caller for a case that cannot occur.
    serde_json::to_string(&file).expect("Template/NamedPreset are always JSON-serializable")
}

/// The inverse of `serialize_presets`. An empty or corrupt `json` (e.g.
/// the `GSettings` key's own un-set default, or a value from a future,
/// incompatible ScreenForge version) is reported as an error rather than
/// silently discarding whatever the user had saved — callers should
/// surface this rather than treat it as "no presets yet".
pub fn deserialize_presets(json: &str) -> Result<Vec<NamedPreset>, PresetError> {
    let file: PresetListFile = serde_json::from_str(json)?;
    match file.version {
        1 => Ok(file.presets),
        other => Err(PresetError::UnsupportedVersion { found: other, supported: CURRENT_VERSION }),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct LabelStyleFile {
    format: String,
    version: u32,
    style: LabelStyle,
}

#[derive(Debug, Error)]
pub enum LabelStyleError {
    #[error("the saved default label style is damaged or not valid ScreenForge JSON: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("unsupported default-label-style format version {found} (this version of ScreenForge supports up to {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
}

/// Serializes the app-wide default label style — the top tier of the
/// three-level label configuration (spec: "globale Anwendungseinstellungen
/// → Projekt-Defaults → Screenshot-Overrides") — to one JSON string for
/// `app/src/main.rs` to write into the `default-label-style` `GSettings`
/// key. Used *only* to seed `Document::label_defaults` when a brand-new
/// project is created (`EditorState::new`); changing it afterward never
/// touches any already-open or already-saved project, since a `Document`
/// carries its own independent `label_defaults` copy from that point on.
pub fn serialize_label_style(style: &LabelStyle) -> String {
    let file = LabelStyleFile { format: "screenforge-label-style".to_string(), version: CURRENT_VERSION, style: style.clone() };
    serde_json::to_string(&file).expect("LabelStyle is always JSON-serializable")
}

/// The inverse of `serialize_label_style`. An empty or corrupt `json`
/// (e.g. the `GSettings` key's own un-set default, or a value from a
/// future, incompatible ScreenForge version) is reported as an error
/// rather than silently discarding a customized global default — callers
/// should fall back to [`LabelStyle::default`] themselves on `Err`, the
/// same way `app/src/main.rs`'s `load_presets` falls back to an empty
/// list.
pub fn deserialize_label_style(json: &str) -> Result<LabelStyle, LabelStyleError> {
    let file: LabelStyleFile = serde_json::from_str(json)?;
    match file.version {
        1 => Ok(file.style),
        other => Err(LabelStyleError::UnsupportedVersion { found: other, supported: CURRENT_VERSION }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_renders_at_the_requested_width() {
        let doc = Document::new();
        let preview = render_preview(&Template::from_document(&doc), 160).unwrap();
        assert_eq!(preview.width(), 160);
        assert!(preview.height() > 0);
    }
    use crate::model::{ImageSource, LayoutMode, Rgba, ScreenshotElement};
    use std::path::PathBuf;

    #[test]
    fn from_document_captures_style_but_not_elements() {
        let mut doc = Document::new();
        doc.layout = LayoutSettings { mode: LayoutMode::Grid, spacing_px: 10.0, margin_x: 20.0, margin_y: 20.0 };
        doc.background = Background::Solid(Rgba::new(0.1, 0.2, 0.3, 1.0));
        let mut el = ScreenshotElement::new(ImageSource::Path(PathBuf::from("a.png")), 100.0, 200.0);
        el.shadow = ShadowParams::strong();
        el.corner_radius = CornerRadius::uniform(12.0);
        doc.elements.push(el);

        let template = Template::from_document(&doc);

        assert_eq!(template.layout.mode, LayoutMode::Grid);
        assert_eq!(template.layout.spacing_px, 10.0);
        assert_eq!(template.shadow, ShadowParams::strong());
        assert_eq!(template.corner_radius, CornerRadius::uniform(12.0));
        match template.background {
            Background::Solid(c) => assert_eq!(c, Rgba::new(0.1, 0.2, 0.3, 1.0)),
            other => panic!("expected Solid background, got {other:?}"),
        }
    }

    #[test]
    fn from_document_uses_defaults_when_there_are_no_elements() {
        let doc = Document::new();
        let template = Template::from_document(&doc);
        assert_eq!(template.shadow, ShadowParams::default());
        assert_eq!(template.corner_radius, CornerRadius::default());
    }

    #[test]
    fn from_document_captures_the_documents_label_defaults() {
        let mut doc = Document::new();
        doc.label_defaults.padding_x = 42.0;
        let template = Template::from_document(&doc);
        assert_eq!(template.label_defaults.padding_x, 42.0);
    }

    #[test]
    fn serialize_then_deserialize_round_trips_a_named_list() {
        let mut doc = Document::new();
        doc.layout = LayoutSettings { mode: LayoutMode::Vertical, spacing_px: 16.0, margin_x: 32.0, margin_y: 32.0 };
        doc.background = Background::Solid(Rgba::new(0.5, 0.5, 0.5, 1.0));
        let presets = vec![
            NamedPreset { name: "Blog".to_string(), template: Template::from_document(&doc) },
            NamedPreset { name: "Standard".to_string(), template: Template::from_document(&Document::new()) },
        ];

        let json = serialize_presets(&presets);
        let loaded = deserialize_presets(&json).unwrap();

        assert_eq!(loaded, presets);
    }

    #[test]
    fn deserializing_garbage_is_reported_as_a_parse_error_not_a_panic() {
        let err = deserialize_presets("not json").unwrap_err();
        assert!(matches!(err, PresetError::Parse(_)));
    }

    #[test]
    fn deserializing_an_empty_string_is_reported_as_a_parse_error() {
        // The `GSettings` key's own un-set state — callers (`PresetStore`)
        // are responsible for treating that as "no presets yet" rather
        // than calling this at all; this function itself must not panic.
        let err = deserialize_presets("").unwrap_err();
        assert!(matches!(err, PresetError::Parse(_)));
    }

    #[test]
    fn unsupported_future_version_is_reported_clearly() {
        let template = Template::from_document(&Document::new());
        let json = serde_json::to_string(&PresetListFile {
            format: "screenforge-presets".into(),
            version: 99,
            presets: vec![NamedPreset { name: "X".to_string(), template }],
        })
        .unwrap();

        let err = deserialize_presets(&json).unwrap_err();
        match err {
            PresetError::UnsupportedVersion { found, supported } => {
                assert_eq!(found, 99);
                assert_eq!(supported, CURRENT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn label_style_serialize_then_deserialize_round_trips() {
        let style = LabelStyle { padding_x: 42.0, ..LabelStyle::label_default() };
        let json = serialize_label_style(&style);
        let loaded = deserialize_label_style(&json).unwrap();
        assert_eq!(loaded, style);
    }

    #[test]
    fn label_style_deserializing_an_empty_string_is_a_parse_error_not_a_panic() {
        let err = deserialize_label_style("").unwrap_err();
        assert!(matches!(err, LabelStyleError::Parse(_)));
    }

    #[test]
    fn label_style_unsupported_future_version_is_reported_clearly() {
        let json = serde_json::to_string(&LabelStyleFile {
            format: "screenforge-label-style".into(),
            version: 99,
            style: LabelStyle::label_default(),
        })
        .unwrap();

        let err = deserialize_label_style(&json).unwrap_err();
        match err {
            LabelStyleError::UnsupportedVersion { found, supported } => {
                assert_eq!(found, 99);
                assert_eq!(supported, CURRENT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }
}
