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
    let project: ProjectFile = serde_json::from_str(&content)?;
    match project.version {
        1 => Ok(project.document),
        other => Err(ProjectError::UnsupportedVersion { found: other, supported: CURRENT_VERSION }),
    }
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
