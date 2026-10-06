//! The scene library: every composition the user works on, kept by the app
//! itself instead of as loose files. Each scene is a folder under the
//! library root:
//!
//! ```text
//! <root>/<id>/scene.screenforge   the project (see `crate::project`)
//! <root>/<id>/preview.png         a small rendering for the overview
//! <root>/<id>/meta.json           name, dates, a short summary
//! ```
//!
//! Deleting moves the folder to `<root>/.trash/`, so it can be undone until
//! the library is opened the next time ([`Library::open`] empties the
//! trash). Plain files and folders, no GTK: the app renders previews and
//! decides names; this module only stores.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::model::Document;
use crate::project::{self, ProjectError};

const SCENE_FILE: &str = "scene.screenforge";
const PREVIEW_FILE: &str = "preview.png";
const META_FILE: &str = "meta.json";
const TRASH_DIR: &str = ".trash";

/// What the overview shows about a scene without opening it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneMeta {
    pub id: Uuid,
    pub name: String,
    /// Seconds since the Unix epoch.
    pub created: u64,
    pub modified: u64,
    pub screenshots: u32,
    /// Output size in pixels, `0` before the first save.
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Project(#[from] ProjectError),
    #[error("{0}")]
    Meta(#[from] serde_json::Error),
    #[error("scene not found")]
    NotFound,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub struct Library {
    root: PathBuf,
}

impl Library {
    /// Opens (creating if needed) the library at `root` and empties the
    /// trash left from the last session.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, LibraryError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        let trash = root.join(TRASH_DIR);
        if trash.exists() {
            fs::remove_dir_all(&trash)?;
        }
        Ok(Self { root })
    }

    /// The library at `root` without touching the trash, for code that
    /// runs after [`Library::open`] was called once at startup.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn dir(&self, id: Uuid) -> PathBuf {
        self.root.join(id.to_string())
    }

    pub fn scene_file(&self, id: Uuid) -> PathBuf {
        self.dir(id).join(SCENE_FILE)
    }

    pub fn preview_file(&self, id: Uuid) -> PathBuf {
        self.dir(id).join(PREVIEW_FILE)
    }

    fn write_meta(&self, meta: &SceneMeta) -> Result<(), LibraryError> {
        let path = self.dir(meta.id).join(META_FILE);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(meta)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn meta(&self, id: Uuid) -> Result<SceneMeta, LibraryError> {
        let bytes = fs::read(self.dir(id).join(META_FILE)).map_err(|_| LibraryError::NotFound)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Every scene that has been saved at least once, most recently changed
    /// first. Folders without a readable meta file are skipped.
    pub fn list(&self) -> Vec<SceneMeta> {
        let mut scenes: Vec<SceneMeta> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| Uuid::parse_str(&entry.file_name().to_string_lossy()).ok())
            .filter(|id| self.scene_file(*id).exists())
            .filter_map(|id| self.meta(id).ok())
            .collect();
        scenes.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.cmp(&b.name)));
        scenes
    }

    /// A new, still empty scene. Nothing is listed until the first
    /// [`Library::save`].
    pub fn create(&self, name: &str) -> Result<SceneMeta, LibraryError> {
        let id = Uuid::new_v4();
        fs::create_dir_all(self.dir(id))?;
        let t = now();
        let meta = SceneMeta { id, name: name.to_string(), created: t, modified: t, screenshots: 0, width: 0, height: 0 };
        self.write_meta(&meta)?;
        Ok(meta)
    }

    /// Writes `doc` (and, if given, a PNG preview) into scene `id`,
    /// replacing the previous state atomically, and updates its summary.
    pub fn save(&self, id: Uuid, doc: &Document, preview_png: Option<&[u8]>) -> Result<SceneMeta, LibraryError> {
        let dir = self.dir(id);
        fs::create_dir_all(&dir)?;
        let mut meta = self.meta(id)?;
        let tmp = dir.join("scene.screenforge.tmp");
        project::save(doc, &tmp)?;
        fs::rename(&tmp, self.scene_file(id))?;
        if let Some(png) = preview_png {
            let tmp = dir.join("preview.png.tmp");
            fs::write(&tmp, png)?;
            fs::rename(tmp, self.preview_file(id))?;
        }
        let c = doc.canvas;
        let scale = c.export_target_width as f64 / c.export_width.max(1) as f64;
        meta.modified = now().max(meta.modified);
        meta.screenshots = doc.elements.len() as u32;
        meta.width = c.export_target_width;
        meta.height = (c.export_height as f64 * scale).round() as u32;
        self.write_meta(&meta)?;
        Ok(meta)
    }

    /// Loads scene `id`; embedded images are extracted into `extract_dir`.
    pub fn load(&self, id: Uuid, extract_dir: &Path) -> Result<Document, LibraryError> {
        let file = self.scene_file(id);
        if !file.exists() {
            return Err(LibraryError::NotFound);
        }
        Ok(project::load(&file, extract_dir)?)
    }

    pub fn rename(&self, id: Uuid, name: &str) -> Result<SceneMeta, LibraryError> {
        let mut meta = self.meta(id)?;
        meta.name = name.to_string();
        self.write_meta(&meta)?;
        Ok(meta)
    }

    /// A copy of scene `id` named `name`, with fresh dates.
    pub fn duplicate(&self, id: Uuid, name: &str) -> Result<SceneMeta, LibraryError> {
        let source = self.meta(id)?;
        let copy = self.create(name)?;
        fs::copy(self.scene_file(id), self.scene_file(copy.id))?;
        if self.preview_file(id).exists() {
            fs::copy(self.preview_file(id), self.preview_file(copy.id))?;
        }
        let meta = SceneMeta { id: copy.id, name: name.to_string(), created: copy.created, modified: copy.modified, ..source };
        self.write_meta(&meta)?;
        Ok(meta)
    }

    /// Moves scene `id` to the trash; [`Library::restore`] brings it back.
    pub fn delete(&self, id: Uuid) -> Result<(), LibraryError> {
        let trash = self.root.join(TRASH_DIR);
        fs::create_dir_all(&trash)?;
        let target = trash.join(id.to_string());
        if target.exists() {
            fs::remove_dir_all(&target)?;
        }
        fs::rename(self.dir(id), target)?;
        Ok(())
    }

    pub fn restore(&self, id: Uuid) -> Result<(), LibraryError> {
        fs::rename(self.root.join(TRASH_DIR).join(id.to_string()), self.dir(id))?;
        Ok(())
    }

    /// Copies a `.screenforge` file in as a new scene named `name`. The
    /// preview follows on the scene's first save.
    pub fn import(&self, file: &Path, name: &str) -> Result<SceneMeta, LibraryError> {
        let meta = self.create(name)?;
        fs::copy(file, self.scene_file(meta.id))?;
        Ok(meta)
    }

    /// Copies scene `id` out as a `.screenforge` file.
    pub fn export(&self, id: Uuid, dest: &Path) -> Result<(), LibraryError> {
        fs::copy(self.scene_file(id), dest)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ImageSource, ScreenshotElement};

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("screenforge-library-test-{name}-{}", Uuid::new_v4()));
        fs::remove_dir_all(&root).ok();
        root
    }

    fn doc_with(n: usize) -> Document {
        let mut doc = Document::new();
        for _ in 0..n {
            doc.elements.push(ScreenshotElement::new(ImageSource::Path("/nonexistent/x.png".into()), 100.0, 200.0));
        }
        doc
    }

    #[test]
    fn a_scene_is_listed_only_after_its_first_save() {
        let root = temp_root("list");
        let lib = Library::open(&root).unwrap();
        let meta = lib.create("Erste").unwrap();
        assert!(lib.list().is_empty());
        let saved = lib.save(meta.id, &doc_with(2), Some(b"png")).unwrap();
        assert_eq!(saved.screenshots, 2);
        let listed = lib.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Erste");
        assert_eq!(fs::read(lib.preview_file(meta.id)).unwrap(), b"png");
        let loaded = lib.load(meta.id, &root.join("extract")).unwrap();
        assert_eq!(loaded.elements.len(), 2);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rename_duplicate_delete_and_restore() {
        let root = temp_root("ops");
        let lib = Library::open(&root).unwrap();
        let a = lib.create("A").unwrap();
        lib.save(a.id, &doc_with(1), None).unwrap();
        lib.rename(a.id, "Alpha").unwrap();
        let b = lib.duplicate(a.id, "Alpha (Kopie)").unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(b.screenshots, 1);
        let mut names: Vec<String> = lib.list().into_iter().map(|m| m.name).collect();
        names.sort();
        assert_eq!(names, vec!["Alpha", "Alpha (Kopie)"]);
        lib.delete(a.id).unwrap();
        assert_eq!(lib.list().len(), 1);
        lib.restore(a.id).unwrap();
        assert_eq!(lib.list().len(), 2);
        lib.delete(b.id).unwrap();
        // Reopening empties the trash for good.
        let lib = Library::open(&root).unwrap();
        assert_eq!(lib.list().len(), 1);
        assert!(lib.restore(b.id).is_err());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn import_and_export_round_trip() {
        let root = temp_root("io");
        let lib = Library::open(&root).unwrap();
        let file = root.join("outside.screenforge");
        project::save(&doc_with(3), &file).unwrap();
        let imported = lib.import(&file, "Importiert").unwrap();
        assert_eq!(lib.load(imported.id, &root.join("x")).unwrap().elements.len(), 3);
        let out = root.join("back.screenforge");
        lib.export(imported.id, &out).unwrap();
        assert!(out.exists());
        fs::remove_dir_all(root).ok();
    }
}
