//! Scenes: every composition is kept in the app's scene library
//! (`screenforge_core::library`) and saves itself. This module holds the
//! autosave, the scene overview (the navigation root, a grid of preview
//! cards), the scene menu behind the editor's title, and the actions that
//! open, create, rename, duplicate, delete, import and export scenes.

use std::cell::Cell;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use screenforge_core::library::{Library, SceneMeta};

use crate::*;

/// Width of the preview pictures stored with each scene.
const PREVIEW_WIDTH: u32 = 560;

/// Width of a card in the overview, in pixels.
const CARD_WIDTH: i32 = 240;

pub(crate) fn library_root() -> PathBuf {
    glib::user_data_dir().join("screenforge").join("scenes")
}

fn library() -> Library {
    Library::at(library_root())
}

/// Where a scene's embedded images are unpacked while it is open. One
/// scene at a time: opening another clears it.
fn assets_dir() -> PathBuf {
    glib::user_cache_dir().join("screenforge").join("scene-assets")
}

fn document_hash(doc: &Document) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(doc).unwrap_or_default().hash(&mut hasher);
    hasher.finish()
}

/// A name for a new scene: its first label, or the date.
fn default_scene_name(doc: &Document) -> String {
    let label = doc
        .elements
        .iter()
        .filter(|e| e.label.enabled)
        .filter_map(|e| e.label.content.lines().next())
        .map(str::trim)
        .find(|l| !l.is_empty());
    match label {
        Some(l) => l.chars().take(48).collect(),
        None => {
            let date = glib::DateTime::now_local().ok().and_then(|d| d.format("%x").ok()).map(|s| s.to_string()).unwrap_or_default();
            // Translators: default name of a new scene; {date} is today's date.
            gettext("Scene from {date}").replace("{date}", &date)
        }
    }
}

/// The scene's name as a file name stem, for exports.
pub(crate) fn file_stem_for_scene(state: &EditorState) -> String {
    let name = state.scene.as_ref().map(|s| s.name.as_str()).unwrap_or("screenforge");
    let cleaned: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '-' }).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() { "screenforge".to_string() } else { cleaned }
}

/// "5 min ago", "yesterday", or a date.
fn relative_time(seconds: u64) -> String {
    let now = glib::DateTime::now_local().map(|d| d.to_unix() as u64).unwrap_or(seconds);
    let diff = now.saturating_sub(seconds);
    match diff {
        0..=59 => gettext("just now"),
        60..=3599 => {
            let m = (diff / 60) as u32;
            ngettext("{n} minute ago", "{n} minutes ago", m).replace("{n}", &m.to_string())
        }
        3600..=86_399 => {
            let h = (diff / 3600) as u32;
            ngettext("{n} hour ago", "{n} hours ago", h).replace("{n}", &h.to_string())
        }
        86_400..=172_799 => gettext("yesterday"),
        172_800..=604_799 => {
            let d = (diff / 86_400) as u32;
            ngettext("{n} day ago", "{n} days ago", d).replace("{n}", &d.to_string())
        }
        _ => glib::DateTime::from_unix_local(seconds as i64).ok().and_then(|d| d.format("%x").ok()).map(|s| s.to_string()).unwrap_or_default(),
    }
}

fn scene_summary(meta: &SceneMeta) -> String {
    let when = relative_time(meta.modified);
    if meta.screenshots == 0 {
        return when;
    }
    let count = ngettext("{n} screenshot", "{n} screenshots", meta.screenshots).replace("{n}", &meta.screenshots.to_string());
    format!("{when} · {count}")
}

fn toast(window: &Window, text: &str) {
    window.toast_overlay().add_toast(adw::Toast::new(text));
}

// ---------------------------------------------------------------- saving

thread_local! {
    static PENDING_SAVE: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
    static GALLERY: RefCell<Option<Rc<Gallery>>> = const { RefCell::new(None) };
}

/// Scene id, document, decoded screenshots, decoded background image and
/// the document's hash: everything a background save needs.
type PendingSave = (Uuid, Document, HashMap<Uuid, DecodedImage>, Option<DecodedImage>, u64);

/// What a save needs, taken from the state on the main thread. `None` when
/// there is nothing to write. Creates the scene on the first save of a new
/// composition.
fn prepare_save(state: &Rc<RefCell<EditorState>>) -> Option<PendingSave> {
    let (empty, has_scene, hash, saved) = {
        let s = state.borrow();
        (s.document.elements.is_empty(), s.scene.is_some(), document_hash(&s.document), s.saved_hash)
    };
    if (empty && !has_scene) || saved == Some(hash) {
        return None;
    }
    if !has_scene {
        let name = default_scene_name(&state.borrow().document);
        let meta = library().create(&name).ok()?;
        state.borrow_mut().scene = Some(meta);
    }
    let id = state.borrow().scene.as_ref()?.id;
    let (doc, decoded, background) = export_inputs(state);
    Some((id, doc, decoded, background, hash))
}

fn write_scene(id: Uuid, doc: &Document, decoded: &HashMap<Uuid, DecodedImage>, background: Option<&DecodedImage>) -> Result<SceneMeta, String> {
    let preview = if doc.elements.is_empty() { None } else { export::render_preview_png(doc, decoded, background, PREVIEW_WIDTH).ok() };
    library().save(id, doc, preview.as_deref()).map_err(|e| e.to_string())
}

fn finish_save(window: &Window, state: &Rc<RefCell<EditorState>>, hash: u64, result: Result<SceneMeta, String>) {
    let again = {
        let mut s = state.borrow_mut();
        s.saving = false;
        match result {
            Ok(meta) => {
                // A rename while saving must not be undone by the save.
                if let Some(current) = s.scene.as_mut().filter(|c| c.id == meta.id) {
                    let name = current.name.clone();
                    *current = SceneMeta { name, ..meta };
                }
                s.saved_hash = Some(hash);
            }
            Err(err) => {
                drop(s);
                toast(window, &gettext("Saving the scene failed: {err}").replace("{err}", &err));
                return;
            }
        }
        std::mem::take(&mut s.save_again)
    };
    let model = state.borrow().model.clone();
    model.update_from(&state.borrow());
    if again {
        schedule_autosave(window, state);
    }
}

/// Saves the document into its scene a second after the last change.
/// Called from `refresh_canvas`, which every edit passes through.
pub(crate) fn schedule_autosave(window: &Window, state: &Rc<RefCell<EditorState>>) {
    {
        let s = state.borrow();
        if s.document.elements.is_empty() && s.scene.is_none() {
            return;
        }
    }
    PENDING_SAVE.with(|p| {
        if let Some(id) = p.borrow_mut().take() {
            id.remove();
        }
    });
    let id = glib::timeout_add_local_once(
        Duration::from_millis(1000),
        glib::clone!(
            #[weak]
            window,
            #[strong]
            state,
            move || {
                PENDING_SAVE.with(|p| p.borrow_mut().take());
                save_in_background(&window, &state);
            }
        ),
    );
    PENDING_SAVE.with(|p| *p.borrow_mut() = Some(id));
}

fn save_in_background(window: &Window, state: &Rc<RefCell<EditorState>>) {
    if state.borrow().saving {
        state.borrow_mut().save_again = true;
        return;
    }
    let Some((id, doc, decoded, background, hash)) = prepare_save(state) else { return };
    state.borrow_mut().saving = true;
    let window = window.clone();
    let state = state.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || write_scene(id, &doc, &decoded, background.as_ref()))
            .await
            .unwrap_or_else(|_| Err("cancelled".to_string()));
        finish_save(&window, &state, hash, result);
    });
}

/// Writes pending changes right now, waiting for a running background
/// save first. Before switching scenes and when the window closes.
pub(crate) fn flush_save(window: &Window, state: &Rc<RefCell<EditorState>>) {
    PENDING_SAVE.with(|p| {
        if let Some(id) = p.borrow_mut().take() {
            id.remove();
        }
    });
    let context = glib::MainContext::default();
    while state.borrow().saving {
        context.iteration(true);
    }
    state.borrow_mut().save_again = false;
    if let Some((id, doc, decoded, background, hash)) = prepare_save(state) {
        let result = write_scene(id, &doc, &decoded, background.as_ref());
        finish_save(window, state, hash, result);
    }
}

// ------------------------------------------------------- opening scenes

fn show_editor(window: &Window) {
    let nav = window.navigation_view();
    if nav.visible_page().and_then(|p| p.tag()).as_deref() != Some("editor") {
        nav.push_by_tag("editor");
    }
}

fn show_scenes(window: &Window) {
    window.navigation_view().pop_to_tag("scenes");
}

/// Replaces the editor's document with `doc`, belonging to `scene`.
fn install_scene(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, doc: Document, scene: Option<SceneMeta>) -> u32 {
    let hash = document_hash(&doc);
    {
        let mut s = state.borrow_mut();
        s.scene = scene;
        s.saved_hash = Some(hash);
    }
    let missing = install_document(window, canvas, state, doc);
    let hash = document_hash(&state.borrow().document);
    state.borrow_mut().saved_hash = Some(hash);
    missing
}

pub(crate) fn open_scene(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, id: Uuid) {
    let current = state.borrow().scene.as_ref().map(|s| s.id);
    if current == Some(id) {
        show_editor(window);
        return;
    }
    flush_save(window, state);
    let lib = library();
    let dir = assets_dir();
    std::fs::remove_dir_all(&dir).ok();
    match (lib.meta(id), lib.load(id, &dir)) {
        (Ok(meta), Ok(doc)) => {
            let missing = install_scene(window, canvas, state, doc, Some(meta));
            show_editor(window);
            if missing > 0 {
                toast(window, &ngettext("{missing} image is missing", "{missing} images are missing", missing).replace("{missing}", &missing.to_string()));
            }
        }
        (_, Err(err)) | (Err(err), _) => toast(window, &gettext("The scene could not be opened: {err}").replace("{err}", &err.to_string())),
    }
}

fn new_scene(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    flush_save(window, state);
    let doc = EditorState::new().document;
    install_scene(window, canvas, state, doc, None);
    state.borrow_mut().saved_hash = None;
    show_editor(window);
}

/// Imports `.screenforge` files as scenes, with previews, and opens the
/// last one.
pub(crate) fn import_scene_files(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, files: Vec<PathBuf>) {
    let lib = library();
    let scratch = glib::user_cache_dir().join("screenforge").join("scene-import");
    let mut last = None;
    for file in files {
        let name = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| gettext("Imported Scene"));
        let meta = match lib.import(&file, &name) {
            Ok(meta) => meta,
            Err(err) => {
                toast(window, &gettext("{file} could not be imported: {err}").replace("{file}", &name).replace("{err}", &err.to_string()));
                continue;
            }
        };
        // Load once to store the summary and a preview.
        std::fs::remove_dir_all(&scratch).ok();
        if let Ok(doc) = lib.load(meta.id, &scratch) {
            let mut decoded = HashMap::new();
            for el in &doc.elements {
                if let ImageSource::Path(path) = &el.source {
                    if let Ok(image) = import::decode_image(path) {
                        decoded.insert(el.id, image);
                    }
                }
            }
            let background = background_image_path(&doc.background).and_then(|p| import::decode_image(&p).ok());
            write_scene(meta.id, &doc, &decoded, background.as_ref()).ok();
        }
        last = Some(meta.id);
    }
    std::fs::remove_dir_all(&scratch).ok();
    refresh_gallery();
    if let Some(id) = last {
        open_scene(window, canvas, state, id);
    }
}

// -------------------------------------------------------- scene commands

fn rename_scene(window: &Window, state: &Rc<RefCell<EditorState>>, id: Uuid, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    match library().rename(id, name) {
        Ok(meta) => {
            if let Some(current) = state.borrow_mut().scene.as_mut().filter(|c| c.id == id) {
                current.name = meta.name.clone();
            }
            let model = state.borrow().model.clone();
            model.update_from(&state.borrow());
            refresh_gallery();
        }
        Err(err) => toast(window, &gettext("Renaming failed: {err}").replace("{err}", &err.to_string())),
    }
}

async fn ask_for_name(window: &Window, heading: &str, initial: &str, accept: &str) -> Option<String> {
    let entry = gtk4::Entry::builder().text(initial).activates_default(true).build();
    let dialog = adw::AlertDialog::builder().heading(heading).extra_child(&entry).default_response("ok").close_response("cancel").build();
    dialog.add_response("cancel", &gettext("Cancel"));
    dialog.add_response("ok", accept);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    entry.grab_focus();
    entry.select_region(0, -1);
    let response = dialog.choose_future(Some(window)).await;
    (response == "ok").then(|| entry.text().trim().to_string()).filter(|t| !t.is_empty())
}

fn duplicate_scene(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, id: Uuid, open: bool) {
    flush_save(window, state);
    let lib = library();
    let Ok(meta) = lib.meta(id) else { return };
    // Translators: name of a duplicated scene.
    let name = gettext("{name} (Copy)").replace("{name}", &meta.name);
    match lib.duplicate(id, &name) {
        Ok(copy) => {
            refresh_gallery();
            if open {
                open_scene(window, canvas, state, copy.id);
                toast(window, &gettext("Copy created"));
            }
        }
        Err(err) => toast(window, &gettext("Duplicating failed: {err}").replace("{err}", &err.to_string())),
    }
}

fn delete_scenes(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, ids: Vec<Uuid>) {
    if ids.is_empty() {
        return;
    }
    flush_save(window, state);
    let lib = library();
    let names: Vec<String> = ids.iter().filter_map(|id| lib.meta(*id).ok()).map(|m| m.name).collect();
    let current = state.borrow().scene.as_ref().map(|s| s.id);
    let mut deleted = Vec::new();
    for id in ids {
        if lib.delete(id).is_ok() {
            deleted.push(id);
        }
    }
    if current.is_some_and(|c| deleted.contains(&c)) {
        let doc = EditorState::new().document;
        install_scene(window, canvas, state, doc, None);
        state.borrow_mut().saved_hash = None;
        show_scenes(window);
    }
    refresh_gallery();
    let title = if deleted.len() == 1 {
        gettext("“{name}” deleted").replace("{name}", names.first().map(String::as_str).unwrap_or(""))
    } else {
        ngettext("{n} scene deleted", "{n} scenes deleted", deleted.len() as u32).replace("{n}", &deleted.len().to_string())
    };
    let toast = adw::Toast::builder().title(title).button_label(gettext("Undo")).timeout(10).build();
    toast.connect_button_clicked(move |_| {
        let lib = library();
        for id in &deleted {
            lib.restore(*id).ok();
        }
        refresh_gallery();
    });
    window.toast_overlay().add_toast(toast);
}

async fn export_scenes(window: &Window, state: &Rc<RefCell<EditorState>>, ids: Vec<Uuid>) {
    flush_save(window, state);
    let lib = library();
    let filter = gtk4::FileFilter::new();
    filter.add_pattern("*.screenforge");
    filter.set_name(Some(&gettext("ScreenForge Scenes")));
    let result = if let [id] = ids.as_slice() {
        let Ok(meta) = lib.meta(*id) else { return };
        let dialog = gtk4::FileDialog::builder()
            .title(gettext("Export Scene"))
            .initial_name(format!("{}.screenforge", meta.name.replace('/', "-")))
            .default_filter(&filter)
            .build();
        let Ok(file) = dialog.save_future(Some(window)).await else { return };
        let Some(path) = file.path() else { return };
        lib.export(*id, &path).map(|_| 1usize)
    } else {
        let dialog = gtk4::FileDialog::builder().title(gettext("Choose a Folder for the Scenes")).build();
        let Ok(folder) = dialog.select_folder_future(Some(window)).await else { return };
        let Some(dir) = folder.path() else { return };
        let mut count = 0;
        let mut result = Ok(0);
        for id in &ids {
            let Ok(meta) = lib.meta(*id) else { continue };
            match lib.export(*id, &dir.join(format!("{}.screenforge", meta.name.replace('/', "-")))) {
                Ok(()) => count += 1,
                Err(err) => result = Err(err),
            }
        }
        result.map(|_: usize| count)
    };
    match result {
        Ok(n) => toast(window, &ngettext("{n} scene exported", "{n} scenes exported", n as u32).replace("{n}", &n.to_string())),
        Err(err) => toast(window, &gettext("Export failed: {err}").replace("{err}", &err.to_string())),
    }
}

/// Applies the look of scene `id` (everything but screenshots and their
/// texts) to the open scene.
fn apply_scene_look(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, id: Uuid) {
    if state.borrow().document.elements.is_empty() {
        toast(window, &gettext("Open a scene with screenshots first"));
        return;
    }
    let scratch = glib::user_cache_dir().join("screenforge").join("scene-look");
    std::fs::remove_dir_all(&scratch).ok();
    match library().load(id, &scratch) {
        Ok(doc) => {
            let template = screenforge_core::template::Template::from_document(&doc);
            apply_preset(window, canvas, state, template);
            show_editor(window);
        }
        Err(err) => toast(window, &gettext("The scene could not be opened: {err}").replace("{err}", &err.to_string())),
    }
    std::fs::remove_dir_all(&scratch).ok();
}

// --------------------------------------------------------------- overview

struct Gallery {
    page: adw::NavigationPage,
    flow: gtk4::FlowBox,
    stack: gtk4::Stack,
    title: adw::WindowTitle,
    search: gtk4::SearchEntry,
    select: gtk4::ToggleButton,
    action_bar: gtk4::ActionBar,
    by_name: Cell<bool>,
    selected: RefCell<HashSet<Uuid>>,
    window: glib::WeakRef<Window>,
    current: RefCell<Option<Uuid>>,
}

fn refresh_gallery() {
    let gallery = GALLERY.with(|g| g.borrow().clone());
    if let Some(gallery) = gallery {
        gallery.refresh();
    }
}

fn card_menu(id: Uuid, current: Option<Uuid>) -> gio::Menu {
    let target = id.to_string();
    let item = |label: &str, action: &str| gio::MenuItem::new(Some(label), Some(&format!("win.{action}::{target}")));
    let menu = gio::Menu::new();
    let open = gio::Menu::new();
    open.append_item(&item(&gettext("Open"), "scene-open"));
    open.append_item(&item(&gettext("Rename…"), "scene-rename"));
    open.append_item(&item(&gettext("Duplicate"), "scene-duplicate"));
    if current.is_some_and(|c| c != id) {
        open.append_item(&item(&gettext("Apply Look to Open Scene"), "scene-apply-look"));
    }
    menu.append_section(None, &open);
    let file = gio::Menu::new();
    file.append_item(&item(&gettext("Export as File…"), "scene-export"));
    menu.append_section(None, &file);
    let danger = gio::Menu::new();
    danger.append_item(&item(&gettext("Delete"), "scene-delete"));
    menu.append_section(None, &danger);
    menu
}

impl Gallery {
    fn selection_mode(&self) -> bool {
        self.select.is_active()
    }

    fn refresh(self: &Rc<Self>) {
        let mut scenes = library().list();
        let total = scenes.len();
        let query = self.search.text().to_lowercase();
        if !query.is_empty() {
            scenes.retain(|m| m.name.to_lowercase().contains(&query));
        }
        if self.by_name.get() {
            scenes.sort_by_key(|m| m.name.to_lowercase());
        }
        let existing: HashSet<Uuid> = scenes.iter().map(|m| m.id).collect();
        self.selected.borrow_mut().retain(|id| existing.contains(id));

        while let Some(child) = self.flow.first_child() {
            self.flow.remove(&child);
        }
        for meta in &scenes {
            self.flow.append(&self.card(meta));
        }
        self.stack.set_visible_child_name(if total == 0 {
            "empty"
        } else if scenes.is_empty() {
            "no-results"
        } else {
            "grid"
        });
        self.update_titles(total);
    }

    fn update_titles(&self, total: usize) {
        let selected = self.selected.borrow().len();
        if self.selection_mode() {
            self.title.set_subtitle(&ngettext("{n} selected", "{n} selected", selected as u32).replace("{n}", &selected.to_string()));
        } else {
            self.title.set_subtitle(&ngettext("{n} scene", "{n} scenes", total as u32).replace("{n}", &total.to_string()));
        }
        self.action_bar.set_revealed(self.selection_mode());
        self.action_bar.set_sensitive(selected > 0);
    }

    fn card(self: &Rc<Self>, meta: &SceneMeta) -> gtk4::FlowBoxChild {
        let id = meta.id;
        let preview = library().preview_file(id);
        let picture: gtk4::Widget = if preview.exists() {
            let picture = gtk4::Picture::for_filename(&preview);
            picture.set_content_fit(gtk4::ContentFit::Contain);
            picture.set_can_shrink(true);
            picture.upcast()
        } else {
            let image = gtk4::Image::from_icon_name("image-x-generic-symbolic");
            image.set_pixel_size(48);
            image.add_css_class("dim-label");
            image.upcast()
        };
        // Fixed 4:3 cards; the clamp keeps a large preview from widening
        // the card.
        let frame = gtk4::AspectFrame::new(0.5, 0.5, 4.0 / 3.0, false);
        frame.set_child(Some(&picture));
        frame.set_size_request(CARD_WIDTH, CARD_WIDTH * 3 / 4);
        let clamp = adw::Clamp::builder().maximum_size(CARD_WIDTH).tightening_threshold(CARD_WIDTH).child(&frame).build();

        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&clamp));
        overlay.add_css_class("card");
        overlay.add_css_class("scene-card");
        overlay.set_overflow(gtk4::Overflow::Hidden);

        let current = *self.current.borrow();
        let more = gtk4::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&card_menu(id, current))
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Start)
            .margin_top(6)
            .margin_end(6)
            .tooltip_text(gettext("Scene Menu"))
            .build();
        more.add_css_class("circular");
        more.add_css_class("scene-card-menu");
        more.set_visible(!self.selection_mode());
        overlay.add_overlay(&more);

        let check = gtk4::CheckButton::builder().halign(gtk4::Align::Start).valign(gtk4::Align::Start).margin_top(8).margin_start(8).build();
        check.add_css_class("selection-mode");
        check.set_active(self.selected.borrow().contains(&id));
        check.set_visible(self.selection_mode());
        overlay.add_overlay(&check);
        check.connect_active_notify(glib::clone!(
            #[weak(rename_to = gallery)]
            self,
            move |check| {
                if check.is_active() {
                    gallery.selected.borrow_mut().insert(id);
                } else {
                    gallery.selected.borrow_mut().remove(&id);
                }
                gallery.update_titles(library().list().len());
            }
        ));

        let click = gtk4::GestureClick::new();
        click.set_button(0);
        click.connect_released(glib::clone!(
            #[weak(rename_to = gallery)]
            self,
            #[weak]
            check,
            #[weak]
            overlay,
            move |gesture, n, x, y| {
                let button = gesture.current_button();
                if button == gdk::BUTTON_SECONDARY {
                    let menu = gtk4::PopoverMenu::from_model(Some(&card_menu(id, *gallery.current.borrow())));
                    menu.set_parent(&overlay);
                    menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                    menu.set_has_arrow(false);
                    menu.connect_closed(|menu| menu.unparent());
                    menu.popup();
                } else if button == gdk::BUTTON_PRIMARY && n == 1 {
                    if gallery.selection_mode() {
                        check.set_active(!check.is_active());
                    } else if let Some(window) = gallery.window.upgrade() {
                        gio::prelude::ActionGroupExt::activate_action(&window, "scene-open", Some(&id.to_string().to_variant()));
                    }
                }
            }
        ));
        overlay.add_controller(click);

        let name = gtk4::Label::builder().label(&meta.name).xalign(0.0).ellipsize(gtk4::pango::EllipsizeMode::End).build();
        name.add_css_class("heading");
        let summary = gtk4::Label::builder().label(scene_summary(meta)).xalign(0.0).ellipsize(gtk4::pango::EllipsizeMode::End).build();
        summary.add_css_class("dim-label");
        summary.add_css_class("caption");

        let column = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        column.set_halign(gtk4::Align::Start);
        column.set_valign(gtk4::Align::Start);
        column.set_size_request(CARD_WIDTH, -1);
        column.append(&overlay);
        column.append(&name);
        column.append(&summary);
        column.set_tooltip_text(Some(&meta.name));

        let child = gtk4::FlowBoxChild::new();
        child.add_css_class("scene-cell");
        child.set_child(Some(&column));
        child.set_focusable(false);
        child
    }
}

fn build_gallery(window: &Window) -> Rc<Gallery> {
    let title = adw::WindowTitle::new(&gettext("Scenes"), "");
    let header = adw::HeaderBar::builder().title_widget(&title).build();

    let new_button = gtk4::Button::builder().action_name("win.new-scene").tooltip_text(gettext("New Scene (Ctrl+N)")).build();
    new_button.set_child(Some(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label(gettext("New Scene")).build()));
    new_button.add_css_class("suggested-action");
    header.pack_start(&new_button);

    let menu = gtk4::MenuButton::builder().icon_name("open-menu-symbolic").tooltip_text(gettext("Main Menu")).primary(true).build();
    menu.set_menu_model(window.primary_menu_button().menu_model().as_ref());
    header.pack_end(&menu);
    let select = gtk4::ToggleButton::builder().icon_name("selection-mode-symbolic").tooltip_text(gettext("Select Scenes")).build();
    header.pack_end(&select);
    let search_toggle = gtk4::ToggleButton::builder().icon_name("system-search-symbolic").tooltip_text(gettext("Search Scenes")).build();
    header.pack_end(&search_toggle);

    let search = gtk4::SearchEntry::builder().placeholder_text(gettext("Search scenes")).hexpand(true).build();
    let search_bar = gtk4::SearchBar::builder().child(&adw::Clamp::builder().maximum_size(480).child(&search).build()).build();
    search_bar.connect_entry(&search);
    search_bar.set_key_capture_widget(Some(window));
    search_toggle.bind_property("active", &search_bar, "search-mode-enabled").bidirectional().build();

    let sort = adw::ToggleGroup::new();
    sort.add(adw::Toggle::builder().label(gettext("Recently Changed")).name("recent").build());
    sort.add(adw::Toggle::builder().label(gettext("Name")).name("name").build());
    sort.set_active_name(Some("recent"));
    sort.set_halign(gtk4::Align::Start);

    let flow = gtk4::FlowBox::builder()
        .selection_mode(gtk4::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(8)
        .column_spacing(18)
        .row_spacing(18)
        .valign(gtk4::Align::Start)
        .build();

    let grid_box = gtk4::Box::new(gtk4::Orientation::Vertical, 18);
    grid_box.set_margin_top(18);
    grid_box.set_margin_bottom(24);
    grid_box.set_margin_start(18);
    grid_box.set_margin_end(18);
    grid_box.append(&sort);
    grid_box.append(&flow);
    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&adw::Clamp::builder().maximum_size(1500).tightening_threshold(1200).child(&grid_box).build())
        .vexpand(true)
        .build();

    let empty_button = gtk4::Button::builder().label(gettext("New Scene")).action_name("win.new-scene").halign(gtk4::Align::Center).build();
    empty_button.add_css_class("pill");
    empty_button.add_css_class("suggested-action");
    let empty = adw::StatusPage::builder()
        .icon_name(APP_ID)
        .title(gettext("No Scenes Yet"))
        .description(gettext("Add screenshots to a new scene. ScreenForge saves every composition here automatically, with all its settings."))
        .child(&empty_button)
        .build();
    let no_results = adw::StatusPage::builder().icon_name("system-search-symbolic").title(gettext("No Matching Scenes")).build();

    let stack = gtk4::Stack::new();
    stack.add_named(&scroller, Some("grid"));
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&no_results, Some("no-results"));

    let action_bar = gtk4::ActionBar::new();
    let duplicate = gtk4::Button::with_label(&gettext("Duplicate"));
    let export = gtk4::Button::with_label(&gettext("Export…"));
    let delete = gtk4::Button::with_label(&gettext("Delete"));
    delete.add_css_class("destructive-action");
    action_bar.pack_start(&duplicate);
    action_bar.pack_start(&export);
    action_bar.pack_end(&delete);
    action_bar.set_revealed(false);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&search_bar);
    toolbar.set_content(Some(&stack));
    toolbar.add_bottom_bar(&action_bar);

    let page = adw::NavigationPage::builder().title(gettext("Scenes")).tag("scenes").child(&toolbar).build();

    let gallery = Rc::new(Gallery {
        page,
        flow,
        stack,
        title,
        search,
        select: select.clone(),
        action_bar,
        by_name: Cell::new(false),
        selected: RefCell::new(HashSet::new()),
        window: window.downgrade(),
        current: RefCell::new(None),
    });

    gallery.search.connect_search_changed(glib::clone!(
        #[weak]
        gallery,
        move |_| gallery.refresh()
    ));
    sort.connect_active_name_notify(glib::clone!(
        #[weak]
        gallery,
        move |sort| {
            gallery.by_name.set(sort.active_name().as_deref() == Some("name"));
            gallery.refresh();
        }
    ));
    select.connect_active_notify(glib::clone!(
        #[weak]
        gallery,
        move |_| {
            gallery.selected.borrow_mut().clear();
            gallery.refresh();
        }
    ));
    for (button, action) in [(&duplicate, "scenes-duplicate"), (&export, "scenes-export"), (&delete, "scenes-delete")] {
        button.connect_clicked(glib::clone!(
            #[weak]
            gallery,
            move |_| {
                let ids: Vec<String> = gallery.selected.borrow().iter().map(Uuid::to_string).collect();
                if let Some(window) = gallery.window.upgrade() {
                    gio::prelude::ActionGroupExt::activate_action(&window, action, Some(&ids.to_variant()));
                }
                gallery.select.set_active(false);
            }
        ));
    }
    gallery
}

// ------------------------------------------------- editor title popover

fn build_title_popover(window: &Window, state: &Rc<RefCell<EditorState>>) {
    let heading = gtk4::Label::builder().label(gettext("Scene")).xalign(0.0).build();
    heading.add_css_class("heading");
    let entry = gtk4::Entry::builder().placeholder_text(gettext("Scene name")).width_chars(26).build();
    let status = gtk4::Label::builder().label(gettext("Saved automatically")).xalign(0.0).build();
    status.add_css_class("dim-label");
    status.add_css_class("caption");
    let button = |label: &str, action: &str| {
        let b = gtk4::Button::builder().label(label).action_name(action).build();
        b.add_css_class("flat");
        if let Some(child) = b.child().and_downcast::<gtk4::Label>() {
            child.set_xalign(0.0);
        }
        b
    };
    let duplicate = button(&gettext("Duplicate"), "win.duplicate-scene");
    let export = button(&gettext("Export as File…"), "win.export-scene");
    let delete = button(&gettext("Delete"), "win.delete-scene");
    delete.add_css_class("error");

    let column = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    column.set_margin_top(6);
    column.set_margin_bottom(6);
    column.set_margin_start(6);
    column.set_margin_end(6);
    for w in [heading.upcast_ref::<gtk4::Widget>(), entry.upcast_ref(), status.upcast_ref()] {
        column.append(w);
    }
    column.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    for w in [&duplicate, &export, &delete] {
        column.append(w);
    }
    let popover = gtk4::Popover::builder().child(&column).build();
    window.scene_title_button().set_popover(Some(&popover));

    let rename = glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[weak]
        entry,
        move || {
            let name = entry.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            let id = state.borrow().scene.as_ref().map(|s| s.id);
            match id {
                Some(id) if state.borrow().scene.as_ref().is_some_and(|s| s.name != name) => rename_scene(&window, &state, id, &name),
                Some(_) => {}
                None => {
                    // Not saved yet: create the scene now so the name sticks.
                    if let Ok(meta) = library().create(&name) {
                        state.borrow_mut().scene = Some(meta);
                        let model = state.borrow().model.clone();
                        model.update_from(&state.borrow());
                        schedule_autosave(&window, &state);
                    }
                }
            }
        }
    );
    entry.connect_activate(glib::clone!(
        #[strong]
        rename,
        #[weak]
        popover,
        move |_| {
            rename();
            popover.popdown();
        }
    ));
    popover.connect_closed(glib::clone!(
        #[strong]
        rename,
        move |_| rename()
    ));
    popover.connect_show(glib::clone!(
        #[strong]
        state,
        #[weak]
        entry,
        #[weak]
        status,
        #[weak]
        delete,
        move |_| {
            let s = state.borrow();
            entry.set_text(&s.scene.as_ref().map(|m| m.name.clone()).unwrap_or_default());
            let saved = s.scene.is_some() && s.saved_hash == Some(document_hash(&s.document));
            status.set_label(&if s.scene.is_none() {
                gettext("Saved automatically once it has a screenshot")
            } else if saved {
                gettext("Saved automatically")
            } else {
                gettext("Saving…")
            });
            delete.set_sensitive(s.scene.is_some());
        }
    ));
}

// --------------------------------------------------------------- wiring

fn uuid_param(parameter: Option<&glib::Variant>) -> Option<Uuid> {
    parameter.and_then(|p| p.get::<String>()).and_then(|s| Uuid::parse_str(&s).ok())
}

fn uuid_list(parameter: Option<&glib::Variant>) -> Vec<Uuid> {
    parameter.and_then(|p| p.get::<Vec<String>>()).unwrap_or_default().iter().filter_map(|s| Uuid::parse_str(s).ok()).collect()
}

/// Builds the scene overview, makes it the navigation root and registers
/// every scene action. Opens the overview when scenes exist, otherwise
/// starts in an empty editor.
pub(crate) fn register_scenes(app: &adw::Application, window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    if let Err(err) = Library::open(library_root()) {
        eprintln!("ScreenForge: could not open the scene library: {err}");
    }
    std::fs::remove_dir_all(assets_dir()).ok();

    let gallery = build_gallery(window);
    GALLERY.with(|g| *g.borrow_mut() = Some(gallery.clone()));
    build_title_popover(window, state);

    let nav = window.navigation_view();
    nav.add(&gallery.page);
    let has_scenes = !library().list().is_empty();
    if has_scenes {
        nav.replace(std::slice::from_ref(&gallery.page));
    } else {
        nav.replace(&[gallery.page.clone(), window.editor_page()]);
    }
    gallery.refresh();

    // Back to the overview: save, then show the current state.
    nav.connect_visible_page_notify(glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[weak]
        gallery,
        move |nav| {
            if nav.visible_page().and_then(|p| p.tag()).as_deref() == Some("scenes") {
                flush_save(&window, &state);
                *gallery.current.borrow_mut() = state.borrow().scene.as_ref().filter(|_| !state.borrow().document.elements.is_empty()).map(|s| s.id);
                gallery.refresh();
            }
        }
    ));

    window.connect_close_request(glib::clone!(
        #[strong]
        state,
        move |window| {
            flush_save(window, &state);
            glib::Propagation::Proceed
        }
    ));

    let simple = |name: &str, f: Rc<dyn Fn()>| {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| f());
        window.add_action(&action);
    };
    simple(
        "new-scene",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move || new_scene(&window, &canvas, &state)
        )),
    );
    simple(
        "show-scenes",
        Rc::new(glib::clone!(
            #[weak]
            window,
            move || show_scenes(&window)
        )),
    );
    simple(
        "save-now",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[strong]
            state,
            move || {
                flush_save(&window, &state);
                if state.borrow().scene.is_some() {
                    toast(&window, &gettext("Scene saved"));
                }
            }
        )),
    );
    simple(
        "duplicate-scene",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move || {
                flush_save(&window, &state);
                let id = state.borrow().scene.as_ref().map(|s| s.id);
                if let Some(id) = id {
                    duplicate_scene(&window, &canvas, &state, id, true);
                }
            }
        )),
    );
    simple(
        "delete-scene",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move || {
                let id = state.borrow().scene.as_ref().map(|s| s.id);
                if let Some(id) = id {
                    delete_scenes(&window, &canvas, &state, vec![id]);
                }
            }
        )),
    );
    simple(
        "export-scene",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[strong]
            state,
            move || {
                flush_save(&window, &state);
                let id = state.borrow().scene.as_ref().map(|s| s.id);
                if let Some(id) = id {
                    let window = window.clone();
                    let state = state.clone();
                    glib::spawn_future_local(async move { export_scenes(&window, &state, vec![id]).await });
                } else {
                    toast(&window, &gettext("Add a screenshot first"));
                }
            }
        )),
    );
    simple(
        "import-scene",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move || {
                let window = window.clone();
                let canvas = canvas.clone();
                let state = state.clone();
                glib::spawn_future_local(async move {
                    let filter = gtk4::FileFilter::new();
                    filter.add_pattern("*.screenforge");
                    filter.set_name(Some(&gettext("ScreenForge Scenes")));
                    let filters = gio::ListStore::new::<gtk4::FileFilter>();
                    filters.append(&filter);
                    let dialog = gtk4::FileDialog::builder().title(gettext("Import Scenes")).accept_label(gettext("Import")).filters(&filters).build();
                    let Ok(files) = dialog.open_multiple_future(Some(&window)).await else { return };
                    let paths: Vec<PathBuf> = (0..files.n_items())
                        .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                        .filter_map(|f| f.path())
                        .collect();
                    import_scene_files(&window, &canvas, &state, paths);
                });
            }
        )),
    );

    let with_id = |name: &str, f: Rc<dyn Fn(Uuid)>| {
        let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
        action.connect_activate(move |_, p| {
            if let Some(id) = uuid_param(p) {
                f(id);
            }
        });
        window.add_action(&action);
    };
    with_id(
        "scene-open",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move |id| open_scene(&window, &canvas, &state, id)
        )),
    );
    with_id(
        "scene-duplicate",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move |id| duplicate_scene(&window, &canvas, &state, id, false)
        )),
    );
    with_id(
        "scene-delete",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move |id| delete_scenes(&window, &canvas, &state, vec![id])
        )),
    );
    with_id(
        "scene-apply-look",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move |id| apply_scene_look(&window, &canvas, &state, id)
        )),
    );
    with_id(
        "scene-export",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[strong]
            state,
            move |id| {
                let window = window.clone();
                let state = state.clone();
                glib::spawn_future_local(async move { export_scenes(&window, &state, vec![id]).await });
            }
        )),
    );
    with_id(
        "scene-rename",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[strong]
            state,
            move |id| {
                let Ok(meta) = library().meta(id) else { return };
                let window = window.clone();
                let state = state.clone();
                glib::spawn_future_local(async move {
                    if let Some(name) = ask_for_name(&window, &gettext("Rename Scene"), &meta.name, &gettext("Rename")).await {
                        rename_scene(&window, &state, id, &name);
                    }
                });
            }
        )),
    );

    let with_ids = |name: &str, f: Rc<dyn Fn(Vec<Uuid>)>| {
        let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING_ARRAY));
        action.connect_activate(move |_, p| f(uuid_list(p)));
        window.add_action(&action);
    };
    with_ids(
        "scenes-delete",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move |ids| delete_scenes(&window, &canvas, &state, ids)
        )),
    );
    with_ids(
        "scenes-duplicate",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move |ids| {
                for id in ids {
                    duplicate_scene(&window, &canvas, &state, id, false);
                }
            }
        )),
    );
    with_ids(
        "scenes-export",
        Rc::new(glib::clone!(
            #[weak]
            window,
            #[strong]
            state,
            move |ids| {
                let window = window.clone();
                let state = state.clone();
                glib::spawn_future_local(async move { export_scenes(&window, &state, ids).await });
            }
        )),
    );

    app.set_accels_for_action("win.new-scene", &["<Ctrl>n"]);
    app.set_accels_for_action("win.duplicate-scene", &["<Ctrl>d"]);
    app.set_accels_for_action("win.export-scene", &["<Ctrl><Shift>s"]);
    app.set_accels_for_action("win.import-scene", &["<Ctrl>i"]);
    app.set_accels_for_action("win.save-now", &["<Ctrl>s"]);
}
