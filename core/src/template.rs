//! Reusable style presets: everything that makes a composition *look* the
//! way it does (layout mode/spacing/margin, background, shadow, corner
//! radius) captured separately from the screenshots themselves, so it can
//! be saved once and reapplied to a different set of images later. JSON
//! (de)serialization mirrors `crate::project` closely — same `version`
//! field for the same forward-migration reason.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{Background, CornerRadius, Document, LayoutSettings, ShadowParams};

pub const CURRENT_VERSION: u32 = 1;

/// A document's style, independent of which screenshots it holds. Shadow
/// and corner radius are captured as a single value rather than
/// per-element, matching the existing assumption elsewhere (spec §27) that
/// these apply uniformly across the whole composition — there's no
/// per-element UI for them yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub layout: LayoutSettings,
    pub background: Background,
    pub shadow: ShadowParams,
    pub corner_radius: CornerRadius,
}

impl Template {
    /// Captures `doc`'s current style. Shadow/corner radius are read from
    /// the first element (if any) — again mirroring the rest of the app's
    /// "uniform across all elements" assumption for these two properties.
    pub fn from_document(doc: &Document) -> Self {
        let (shadow, corner_radius) = match doc.elements.first() {
            Some(el) => (el.shadow, el.corner_radius),
            None => (ShadowParams::default(), CornerRadius::default()),
        };
        Template { layout: doc.layout, background: doc.background.clone(), shadow, corner_radius }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateFile {
    pub format: String,
    pub version: u32,
    pub template: Template,
}

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("could not read template file: {0}")]
    Io(#[from] std::io::Error),
    #[error("template file is damaged or not valid ScreenForge JSON: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("unsupported template format version {found} (this version of ScreenForge supports up to {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
}

pub fn save(template: &Template, path: &Path) -> Result<(), TemplateError> {
    let file = TemplateFile { format: "screenforge-template".to_string(), version: CURRENT_VERSION, template: template.clone() };
    let json = serde_json::to_string_pretty(&file)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn load(path: &Path) -> Result<Template, TemplateError> {
    let content = std::fs::read_to_string(path)?;
    let file: TemplateFile = serde_json::from_str(&content)?;
    match file.version {
        1 => Ok(file.template),
        other => Err(TemplateError::UnsupportedVersion { found: other, supported: CURRENT_VERSION }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ImageSource, LayoutMode, Rgba, ScreenshotElement};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_path(name: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("screenforge-template-test-{}-{n}-{name}", std::process::id()))
    }

    #[test]
    fn from_document_captures_style_but_not_elements() {
        let mut doc = Document::new();
        doc.layout = LayoutSettings { mode: LayoutMode::Grid, spacing_px: 10.0, margin_px: 20.0 };
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
    fn save_then_load_round_trips() {
        let path = temp_path("roundtrip.screenforge-template");
        let mut doc = Document::new();
        doc.layout = LayoutSettings { mode: LayoutMode::Vertical, spacing_px: 16.0, margin_px: 32.0 };
        doc.background = Background::Solid(Rgba::new(0.5, 0.5, 0.5, 1.0));
        let template = Template::from_document(&doc);

        save(&template, &path).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded, template);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_is_reported_as_io_error() {
        let path = temp_path("does-not-exist.screenforge-template");
        let err = load(&path).unwrap_err();
        assert!(matches!(err, TemplateError::Io(_)));
    }

    #[test]
    fn corrupted_file_is_reported_as_a_parse_error_not_a_panic() {
        let path = temp_path("corrupted.screenforge-template");
        std::fs::write(&path, b"not json").unwrap();
        let err = load(&path).unwrap_err();
        assert!(matches!(err, TemplateError::Parse(_)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn unsupported_future_version_is_reported_clearly() {
        let path = temp_path("future.screenforge-template");
        let template = Template::from_document(&Document::new());
        let json =
            serde_json::to_string(&TemplateFile { format: "screenforge-template".into(), version: 99, template }).unwrap();
        std::fs::write(&path, json).unwrap();

        let err = load(&path).unwrap_err();
        match err {
            TemplateError::UnsupportedVersion { found, supported } => {
                assert_eq!(found, 99);
                assert_eq!(supported, CURRENT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
        std::fs::remove_file(&path).ok();
    }
}
