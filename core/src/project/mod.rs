//! `.screenforge` project file (de)serialization. JSON via serde, with a
//! `version` field from day one so a future format change can migrate old
//! files instead of rejecting them outright (spec §13).
//!
//! Image references are path-based for the MVP (spec §16: "Originalbilder
//! sollen nach Möglichkeit nicht unnötig verändert werden") — `Document`
//! already only ever holds `ImageSource::Path` for elements created via
//! import, so saving it directly is enough; `ImageSource::Embedded` exists
//! in the model for the later "save project with assets" option without
//! needing a schema change then.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::Document;

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectFile {
    pub format: String,
    pub version: u32,
    pub document: Document,
}

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("could not read project file: {0}")]
    Io(#[from] std::io::Error),
    #[error("project file is damaged or not valid ScreenForge JSON: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("unsupported project format version {found} (this version of ScreenForge supports up to {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
}

pub fn save(document: &Document, path: &Path) -> Result<(), ProjectError> {
    let project = ProjectFile { format: "screenforge".to_string(), version: CURRENT_VERSION, document: document.clone() };
    let json = serde_json::to_string_pretty(&project)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Loads a project, migrating older format versions forward as needed. Only
/// version 1 exists so far; the `match` is where a future `migrate_v1_to_v2`
/// step would slot in without touching the call sites.
pub fn load(path: &Path) -> Result<Document, ProjectError> {
    let content = std::fs::read_to_string(path)?;
    let mut raw: serde_json::Value = serde_json::from_str(&content)?;
    migrate_legacy_text_overlay(&mut raw);
    let project: ProjectFile = serde_json::from_value(raw)?;
    match project.version {
        1 => {
            let mut document = project.document;
            // Re-fit rather than trust the saved canvas size: it's a
            // derived value (see `fit_canvas_to_content`), and trusting a
            // stale one here is exactly how content used to end up cropped.
            crate::layout::fit_canvas_to_content(&mut document);
            Ok(document)
        }
        other => Err(ProjectError::UnsupportedVersion { found: other, supported: CURRENT_VERSION }),
    }
}

/// Projects saved before the title/label system existed used a simpler
/// `text_overlay` field (`{enabled, content, x, y, font_size, color}`) for
/// what's now `Document.title` (a full `TextElement`). `Document`'s own
/// `#[serde(default)]` on `title` already means an old file loads without
/// error even without this step — but silently as a disabled, empty title,
/// discarding whatever caption was actually saved. This runs on the raw
/// JSON, before the typed deserialization ever sees it, converting that
/// old shape into an equivalent `TextElement` (the same x/y, now as
/// `TextPosition::Absolute`, and the same content/size/color) so the
/// caption survives the upgrade instead of quietly vanishing. A no-op
/// whenever `title` is already present (current-format files, or a file
/// this has already migrated).
fn migrate_legacy_text_overlay(raw: &mut serde_json::Value) {
    let Some(document) = raw.get_mut("document") else { return };
    if document.get("title").is_some() {
        return;
    }
    let Some(text_overlay) = document.get("text_overlay").cloned() else { return };
    let Some(document_obj) = document.as_object_mut() else { return };

    let get_f64 = |key: &str, default: f64| text_overlay.get(key).and_then(|v| v.as_f64()).unwrap_or(default);
    let enabled = text_overlay.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    let content = text_overlay.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let color = text_overlay
        .get("color")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({ "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 }));

    let title = serde_json::json!({
        "enabled": enabled,
        "content": content,
        "position": { "mode": "absolute", "x": get_f64("x", 48.0), "y": get_f64("y", 24.0) },
        "typography": {
            "font_family": "Sans",
            "font_size": get_f64("font_size", 32.0),
            "weight": 700,
            "italic": false,
            "color": color,
            "alignment": "center",
            "opacity": 1.0,
            "letter_spacing": 0.0,
            "line_spacing": 1.2,
            "wrap": false
        },
        "background": { "type": "none" },
        "corner_radius": { "top_left": 0.0, "top_right": 0.0, "bottom_right": 0.0, "bottom_left": 0.0 },
        "background_padding": 16.0,
        "shadow": {
            "enabled": false, "offset_x": 0.0, "offset_y": 0.0, "blur": 0.0, "opacity": 0.0,
            "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 }
        }
    });
    document_obj.insert("title".to_string(), title);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Background, ImageSource, LayoutSettings, Rgba, ScreenshotElement};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_path(name: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("screenforge-test-{}-{n}-{name}", std::process::id()))
    }

    fn sample_document() -> Document {
        let mut doc = Document::new();
        doc.background = Background::Solid(Rgba::new(0.1, 0.2, 0.3, 1.0));
        doc.layout = LayoutSettings { spacing_px: 12.0, margin_px: 30.0, ..LayoutSettings::default() };
        doc.elements.push(ScreenshotElement::new(ImageSource::Path(PathBuf::from("/tmp/a.png")), 400.0, 800.0));
        doc.elements.push(ScreenshotElement::new(ImageSource::Path(PathBuf::from("/tmp/b.png")), 420.0, 820.0));
        doc
    }

    #[test]
    fn save_then_load_round_trips_a_simple_document() {
        let path = temp_path("simple.screenforge");
        let doc = Document::new();
        save(&doc, &path).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.id, doc.id);
        assert_eq!(loaded.layout, doc.layout);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_then_load_round_trips_multiple_elements_and_background() {
        let path = temp_path("multi.screenforge");
        let doc = sample_document();
        save(&doc, &path).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded.elements.len(), 2);
        assert_eq!(loaded.elements[0].natural_width, 400.0);
        assert_eq!(loaded.elements[1].natural_width, 420.0);
        match loaded.background {
            Background::Solid(c) => assert_eq!(c, Rgba::new(0.1, 0.2, 0.3, 1.0)),
            other => panic!("expected Solid background, got {other:?}"),
        }
        assert_eq!(loaded.layout.spacing_px, 12.0);
        assert_eq!(loaded.layout.margin_px, 30.0);
        std::fs::remove_file(&path).ok();
    }

    /// Spec: "the project must be able to regenerate the exact same
    /// background" — this only holds if every generator input (seed,
    /// strategy, resolved palette, and every numeric parameter) actually
    /// survives a save/load round trip, not just some of them.
    #[test]
    fn save_then_load_round_trips_a_generated_background_exactly() {
        use crate::model::{ColorStrategy, GeneratedBackground};

        let path = temp_path("generated-background.screenforge");
        let mut doc = Document::new();
        let generated = GeneratedBackground {
            seed: 48291374,
            color_strategy: ColorStrategy::FromScreenshots,
            palette: vec![Rgba::new(0.2, 0.3, 0.6, 1.0), Rgba::new(0.8, 0.5, 0.1, 1.0)],
            adapt_to_screenshots: false,
            inverse_contrast: 0.73,
            density: 0.61,
            flow: 0.28,
            variation: 0.5,
            contrast: 0.5,
            softness: 0.9,
            corner_bias: 0.44,
            offset_x: -0.2,
            offset_y: 0.15,
            scale: 1.3,
        };
        doc.background = Background::Generated(generated.clone());

        save(&doc, &path).unwrap();
        let loaded = load(&path).unwrap();

        match loaded.background {
            Background::Generated(loaded_generated) => assert_eq!(loaded_generated, generated),
            other => panic!("expected Generated background, got {other:?}"),
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_is_reported_as_io_error() {
        let path = temp_path("does-not-exist.screenforge");
        let err = load(&path).unwrap_err();
        assert!(matches!(err, ProjectError::Io(_)));
    }

    #[test]
    fn corrupted_file_is_reported_as_a_parse_error_not_a_panic() {
        let path = temp_path("corrupted.screenforge");
        std::fs::write(&path, b"{ this is not valid json at all").unwrap();
        let err = load(&path).unwrap_err();
        assert!(matches!(err, ProjectError::Parse(_)));
        std::fs::remove_file(&path).ok();
    }

    /// A 0.1.0-shaped project file predates `Transform::flip_horizontal`/
    /// `flip_vertical` — this pins down that loading one doesn't break just
    /// because `core::model` grew fields since. This is `#[serde(default)]`
    /// doing its job, not a project-format `version` bump, so this test
    /// stays version 1 throughout.
    #[test]
    fn loads_a_pre_flip_fields_transform_with_defaults() {
        let path = temp_path("pre-flip-fields.screenforge");
        let json = r#"{
            "format": "screenforge",
            "version": 1,
            "document": {
                "id": "8f14e45f-ceea-467e-adc0-51944115d5c6",
                "elements": [{
                    "id": "9b2e1a3c-1234-4a3b-8cde-0123456789ab",
                    "source": { "type": "path", "value": "/tmp/a.png" },
                    "natural_width": 400.0,
                    "natural_height": 800.0,
                    "transform": {
                        "x": 0.0, "y": 0.0, "width": 0.0, "height": 0.0,
                        "rotation_deg": 0.0, "aspect_locked": true
                    },
                    "corner_radius": { "top_left": 0.0, "top_right": 0.0, "bottom_right": 0.0, "bottom_left": 0.0 },
                    "shadow": {
                        "enabled": false, "offset_x": 0.0, "offset_y": 0.0, "blur": 0.0, "opacity": 0.0,
                        "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 }
                    },
                    "visible": true
                }],
                "layout": { "mode": "horizontal", "spacing_px": 24.0, "margin_px": 48.0 },
                "background": { "type": "solid", "value": { "r": 0.95, "g": 0.95, "b": 0.96, "a": 1.0 } },
                "canvas": { "export_width": 1920, "export_height": 1080, "export_format": "png", "export_quality": 90 }
            }
        }"#;
        std::fs::write(&path, json).unwrap();

        let doc = load(&path).unwrap();
        assert!(!doc.elements[0].transform.flip_horizontal);
        assert!(!doc.elements[0].transform.flip_vertical);
        std::fs::remove_file(&path).ok();
    }

    /// A project saved before the title/label system existed used the
    /// older, simpler `text_overlay` shape — this pins down that its
    /// caption survives loading (migrated into `Document.title`) instead
    /// of silently vanishing behind `#[serde(default)]`.
    #[test]
    fn migrates_a_legacy_text_overlay_into_the_new_title() {
        let path = temp_path("legacy-text-overlay.screenforge");
        let json = r#"{
            "format": "screenforge",
            "version": 1,
            "document": {
                "id": "8f14e45f-ceea-467e-adc0-51944115d5c6",
                "elements": [],
                "layout": { "mode": "horizontal", "spacing_px": 24.0, "margin_px": 48.0 },
                "background": { "type": "solid", "value": { "r": 0.95, "g": 0.95, "b": 0.96, "a": 1.0 } },
                "canvas": { "export_width": 1920, "export_height": 1080, "export_format": "png", "export_quality": 90 },
                "text_overlay": {
                    "enabled": true,
                    "content": "My App Review",
                    "x": 100.0,
                    "y": 50.0,
                    "font_size": 40.0,
                    "color": { "r": 0.1, "g": 0.2, "b": 0.3, "a": 1.0 }
                }
            }
        }"#;
        std::fs::write(&path, json).unwrap();

        let doc = load(&path).unwrap();
        assert!(doc.title.enabled);
        assert_eq!(doc.title.content, "My App Review");
        assert_eq!(doc.title.position, crate::model::TextPosition::Absolute { x: 100.0, y: 50.0 });
        assert_eq!(doc.title.typography.font_size, 40.0);
        assert_eq!(doc.title.typography.color, crate::model::Rgba::new(0.1, 0.2, 0.3, 1.0));
        std::fs::remove_file(&path).ok();
    }

    /// A file that already carries the new `title` field must not be
    /// clobbered by a leftover/legacy `text_overlay` also present.
    #[test]
    fn a_file_with_both_title_and_legacy_text_overlay_keeps_the_title() {
        let path = temp_path("title-and-legacy.screenforge");
        let json = r#"{
            "format": "screenforge",
            "version": 1,
            "document": {
                "id": "8f14e45f-ceea-467e-adc0-51944115d5c6",
                "elements": [],
                "layout": { "mode": "horizontal", "spacing_px": 24.0, "margin_px": 48.0 },
                "background": { "type": "solid", "value": { "r": 0.95, "g": 0.95, "b": 0.96, "a": 1.0 } },
                "canvas": { "export_width": 1920, "export_height": 1080, "export_format": "png", "export_quality": 90 },
                "text_overlay": { "enabled": true, "content": "Old", "x": 1.0, "y": 1.0, "font_size": 10.0, "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 } },
                "title": {
                    "enabled": true,
                    "content": "New Title",
                    "position": { "mode": "absolute", "x": 5.0, "y": 5.0 },
                    "typography": {
                        "font_family": "Sans", "font_size": 20.0, "weight": 700, "italic": false,
                        "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 }, "alignment": "center", "opacity": 1.0,
                        "letter_spacing": 0.0, "line_spacing": 1.2, "wrap": false
                    },
                    "background": { "type": "none" },
                    "corner_radius": { "top_left": 0.0, "top_right": 0.0, "bottom_right": 0.0, "bottom_left": 0.0 },
                    "background_padding": 16.0,
                    "shadow": { "enabled": false, "offset_x": 0.0, "offset_y": 0.0, "blur": 0.0, "opacity": 0.0, "color": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 } }
                }
            }
        }"#;
        std::fs::write(&path, json).unwrap();

        let doc = load(&path).unwrap();
        assert_eq!(doc.title.content, "New Title");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn unsupported_future_version_is_reported_clearly() {
        let path = temp_path("future.screenforge");
        let doc = Document::new();
        let json = serde_json::to_string(&ProjectFile { format: "screenforge".into(), version: 99, document: doc }).unwrap();
        std::fs::write(&path, json).unwrap();

        let err = load(&path).unwrap_err();
        match err {
            ProjectError::UnsupportedVersion { found, supported } => {
                assert_eq!(found, 99);
                assert_eq!(supported, CURRENT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
        std::fs::remove_file(&path).ok();
    }
}
