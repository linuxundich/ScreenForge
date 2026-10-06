//! Background variants: a grid of six alternatives in the sidebar's
//! background tab, and the larger "Hintergrund wählen" studio dialog with
//! nine big previews per style. Every preview shows the real composition
//! (screenshots, shadows, labels) on top of the candidate background, so
//! the choice is made in context.
//!
//! Candidates come from `screenforge_core::generator::variant`, which is
//! deterministic in the current background's seed plus a roll counter —
//! "Neue Varianten" just bumps the counter.

use crate::*;

use screenforge_core::background_cache::BackgroundCache;
use screenforge_core::generator::variant;
use screenforge_core::shadow_cache::ShadowCache;

const SIDEBAR_VARIANTS: u64 = 6;
const SIDEBAR_THUMB_WIDTH: i32 = 200;
const STUDIO_VARIANTS: u64 = 9;
const STUDIO_THUMB_WIDTH: i32 = 560;

/// `count` candidates derived from `base`, re-resolving each one's palette
/// for its own seed/style/mood unless the colors are the user's manual
/// choice. `roll` selects a fresh, still deterministic set.
fn candidates(state: &mut EditorState, base: &GeneratedBackground, roll: u64, mix_styles: bool, count: u64) -> Vec<GeneratedBackground> {
    (0..count)
        .map(|i| {
            let mut candidate = variant(base, roll * 1000 + i, mix_styles);
            if !matches!(candidate.color_strategy, ColorStrategy::Manual) {
                candidate.palette = resolve_generator_palette(
                    state,
                    candidate.color_strategy,
                    candidate.inverse_contrast,
                    candidate.seed,
                    candidate.style,
                    candidate.mood,
                );
            }
            candidate
        })
        .collect()
}

/// Renders the current document with `background` swapped in, `width`
/// pixels wide, as a texture for a `GtkPicture`. Without `with_elements`
/// only the background is drawn — the small sidebar thumbnails use that,
/// since screenshots would cover most of a thumbnail that size.
fn render_preview(state: &mut EditorState, background: &GeneratedBackground, width: i32, with_elements: bool) -> Option<gdk::Texture> {
    let surfaces = element_surfaces(state);
    let mut doc = state.document.clone();
    doc.background = Background::Generated(background.clone());
    if !with_elements {
        doc.elements.clear();
    }
    let native_w = doc.canvas.export_width.max(1) as f64;
    let native_h = doc.canvas.export_height.max(1) as f64;
    let scale = width as f64 / native_w;
    let height = (native_h * scale).round().max(1.0) as i32;
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).ok()?;
    screenforge_core::render::compose(&doc, &surface, scale, &surfaces, None, &ShadowCache::new(), &BackgroundCache::new()).ok()?;
    surface.flush();
    let stride = surface.stride() as usize;
    let data = surface.data().ok()?.to_vec();
    let bytes = glib::Bytes::from_owned(data);
    Some(gdk::MemoryTexture::new(width, height, gdk::MemoryFormat::B8g8r8a8Premultiplied, &bytes, stride).upcast())
}

/// Commits `new` as the document's background (undoable) and reflects it
/// onto the generator controls.
pub(crate) fn apply_generated_background(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>, new: GeneratedBackground) {
    {
        let mut state_ref = state.borrow_mut();
        let old = state_ref.document.background.clone();
        if old == Background::Generated(new.clone()) {
            return;
        }
        let EditorState { document, undo_stack, .. } = &mut *state_ref;
        undo_stack.apply(Box::new(SetBackground { old, new: Background::Generated(new.clone()) }), document);
        state_ref.syncing_controls = true;
    }
    sync_generator_controls(window, &new);
    state.borrow_mut().syncing_controls = false;
    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

fn thumbnail_button(texture: Option<&gdk::Texture>, height: i32) -> (gtk4::Button, gtk4::Picture) {
    let picture = gtk4::Picture::new();
    picture.set_content_fit(gtk4::ContentFit::Cover);
    picture.set_size_request(-1, height);
    picture.set_paintable(texture);
    picture.add_css_class("variant-picture");
    let button = gtk4::Button::builder().child(&picture).css_classes(["flat", "variant-thumb"]).build();
    (button, picture)
}

/// Refills the sidebar grid from the current background. Hides the grid
/// when the background isn't generated.
pub(crate) fn rebuild_sidebar_variants(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let flow = window.variants_flow();
    while let Some(child) = flow.first_child() {
        flow.remove(&child);
    }
    let Background::Generated(base) = state.borrow().document.background.clone() else { return };
    if state.borrow().document.elements.is_empty() && base.palette.is_empty() {
        return;
    }
    let mix = window.variants_mix_row().is_active();
    let roll = state.borrow().variant_roll;
    let list = candidates(&mut state.borrow_mut(), &base, roll, mix, SIDEBAR_VARIANTS);
    for candidate in list {
        let texture = render_preview(&mut state.borrow_mut(), &candidate, SIDEBAR_THUMB_WIDTH, false);
        let (button, _) = thumbnail_button(texture.as_ref(), 56);
        button.set_tooltip_text(Some(style_name(candidate.style)));
        button.connect_clicked(glib::clone!(
            #[weak]
            window,
            #[weak]
            canvas,
            #[strong]
            state,
            move |_| apply_generated_background(&window, &canvas, &state, candidate.clone())
        ));
        flow.insert(&button, -1);
    }
}

pub(crate) fn style_name(style: GeneratorStyle) -> &'static str {
    match style {
        GeneratorStyle::Layers => "Schichten",
        GeneratorStyle::Arcs => "Bögen",
        GeneratorStyle::Ribbons => "Bänder",
        GeneratorStyle::Planes => "Flächen",
        GeneratorStyle::Lines => "Linien",
        GeneratorStyle::Mist => "Nebel",
        GeneratorStyle::Waves => "Wellen (klassisch)",
    }
}

pub(crate) fn register_variant_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    window.variants_reroll_row().connect_activated(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| {
            state.borrow_mut().variant_roll += 1;
            rebuild_sidebar_variants(&window, &canvas, &state);
        }
    ));
    window.variants_mix_row().connect_active_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| rebuild_sidebar_variants(&window, &canvas, &state)
    ));
    window.variants_studio_row().connect_activated(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| open_studio(&window, &canvas, &state)
    ));
}

/// What the studio dialog shows right now.
struct Studio {
    /// `None` means "all styles mixed".
    style: Option<GeneratorStyle>,
    mood: Mood,
    roll: u64,
    candidates: Vec<GeneratedBackground>,
    selected: Option<usize>,
    /// Bumped on every refill, so a progressive render still running for
    /// an older set stops instead of filling in stale pictures.
    generation: u64,
}

/// The "Hintergrund wählen" dialog: styles on the left, nine large
/// previews on the right, mood and "Neue Varianten" at the bottom.
fn open_studio(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let Background::Generated(base) = state.borrow().document.background.clone() else { return };

    let dialog = adw::Dialog::builder().title("Hintergrund wählen").content_width(1000).content_height(680).build();
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    header.set_show_start_title_buttons(false);
    let cancel = gtk4::Button::with_label("Abbrechen");
    let apply = gtk4::Button::builder().label("Übernehmen").css_classes(["suggested-action"]).sensitive(false).build();
    header.pack_start(&cancel);
    header.pack_end(&apply);
    toolbar.add_top_bar(&header);

    let styles = gtk4::ListBox::new();
    styles.add_css_class("navigation-sidebar");
    let style_options: Vec<Option<GeneratorStyle>> = std::iter::once(None)
        .chain(GeneratorStyle::MODERN.iter().copied().map(Some))
        .chain(std::iter::once(Some(GeneratorStyle::Waves)))
        .collect();
    for option in &style_options {
        let label = gtk4::Label::builder().label(option.map(style_name).unwrap_or("Alle Stile")).xalign(0.0).build();
        styles.append(&label);
    }
    let styles_scroller = gtk4::ScrolledWindow::builder().child(&styles).hscrollbar_policy(gtk4::PolicyType::Never).width_request(190).build();

    let flow = gtk4::FlowBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .activate_on_single_click(true)
        .homogeneous(true)
        .min_children_per_line(3)
        .max_children_per_line(3)
        .column_spacing(12)
        .row_spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .valign(gtk4::Align::Start)
        .build();
    let flow_scroller = gtk4::ScrolledWindow::builder().child(&flow).hexpand(true).vexpand(true).build();

    let body = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    body.append(&styles_scroller);
    body.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
    body.append(&flow_scroller);
    toolbar.set_content(Some(&body));

    let bottom = gtk4::Box::builder().spacing(12).margin_start(12).margin_end(12).margin_top(6).margin_bottom(6).build();
    bottom.append(&gtk4::Label::new(Some("Stimmung")));
    let mood = gtk4::DropDown::from_strings(&["Kräftig", "Hell", "Dunkel"]);
    mood.set_selected(index_for_mood(base.mood));
    bottom.append(&mood);
    let spacer = gtk4::Box::builder().hexpand(true).build();
    bottom.append(&spacer);
    let show_screenshots = gtk4::CheckButton::builder().label("Screenshots zeigen").active(true).build();
    bottom.append(&show_screenshots);
    let reroll = gtk4::Button::builder().label("Neue Varianten").build();
    bottom.append(&reroll);
    toolbar.add_bottom_bar(&bottom);
    dialog.set_child(Some(&toolbar));

    let studio = Rc::new(RefCell::new(Studio { style: Some(base.style), mood: base.mood, roll: 0, candidates: Vec::new(), selected: None, generation: 0 }));

    // Fills the grid with placeholders immediately, then renders one
    // preview per main-loop iteration so the dialog stays responsive.
    let refill: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[weak]
        flow,
        #[weak]
        apply,
        #[strong]
        studio,
        #[strong]
        state,
        #[strong]
        base,
        #[weak]
        show_screenshots,
        move || {
            let with_elements = show_screenshots.is_active();
            while let Some(child) = flow.first_child() {
                flow.remove(&child);
            }
            let (style, mood, roll, generation) = {
                let mut s = studio.borrow_mut();
                s.generation += 1;
                s.selected = None;
                (s.style, s.mood, s.roll, s.generation)
            };
            apply.set_sensitive(false);
            let seeded = GeneratedBackground { style: style.unwrap_or(base.style), mood, ..base.clone() };
            let list = candidates(&mut state.borrow_mut(), &seeded, roll + 1, style.is_none(), STUDIO_VARIANTS);
            let mut pictures = Vec::new();
            for candidate in &list {
                let picture = gtk4::Picture::new();
                picture.set_content_fit(gtk4::ContentFit::Contain);
                picture.set_size_request(-1, 170);
                picture.add_css_class("variant-picture");
                picture.set_tooltip_text(Some(style_name(candidate.style)));
                flow.insert(&picture, -1);
                pictures.push(picture);
            }
            studio.borrow_mut().candidates = list.clone();
            glib::spawn_future_local(glib::clone!(
                #[strong]
                studio,
                #[strong]
                state,
                async move {
                    for (candidate, picture) in list.iter().zip(pictures) {
                        glib::timeout_future(Duration::from_millis(1)).await;
                        if studio.borrow().generation != generation {
                            return;
                        }
                        let texture = render_preview(&mut state.borrow_mut(), candidate, STUDIO_THUMB_WIDTH, with_elements);
                        picture.set_paintable(texture.as_ref());
                    }
                }
            ));
        }
    ));

    let initial_row = style_options.iter().position(|o| *o == Some(base.style)).unwrap_or(0);
    styles.select_row(styles.row_at_index(initial_row as i32).as_ref());
    styles.connect_row_selected(glib::clone!(
        #[strong]
        studio,
        #[strong]
        refill,
        move |_, row| {
            let Some(row) = row else { return };
            studio.borrow_mut().style = style_options.get(row.index() as usize).copied().flatten();
            refill();
        }
    ));
    mood.connect_selected_notify(glib::clone!(
        #[strong]
        studio,
        #[strong]
        refill,
        move |dropdown| {
            studio.borrow_mut().mood = mood_for_index(dropdown.selected());
            refill();
        }
    ));
    show_screenshots.connect_toggled(glib::clone!(
        #[strong]
        refill,
        move |_| refill()
    ));
    reroll.connect_clicked(glib::clone!(
        #[strong]
        studio,
        #[strong]
        refill,
        move |_| {
            studio.borrow_mut().roll += 1;
            refill();
        }
    ));
    flow.connect_child_activated(glib::clone!(
        #[weak]
        apply,
        #[strong]
        studio,
        move |_, child| {
            studio.borrow_mut().selected = Some(child.index() as usize);
            apply.set_sensitive(true);
        }
    ));
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    apply.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        studio,
        #[strong]
        state,
        move |_| {
            let chosen = {
                let s = studio.borrow();
                s.selected.and_then(|i| s.candidates.get(i).cloned())
            };
            if let Some(chosen) = chosen {
                apply_generated_background(&window, &canvas, &state, chosen);
            }
            dialog.close();
        }
    ));

    refill();
    dialog.present(Some(window));
}
