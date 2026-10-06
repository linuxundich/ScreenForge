use crate::*;

pub(crate) fn color_strategy_for_index(index: u32) -> ColorStrategy {
    match index {
        0 => ColorStrategy::Manual,
        1 => ColorStrategy::FromScreenshots,
        2 => ColorStrategy::Grayscale,
        _ => ColorStrategy::Random,
    }
}

pub(crate) fn index_for_color_strategy(strategy: ColorStrategy) -> u32 {
    match strategy {
        ColorStrategy::Manual => 0,
        ColorStrategy::FromScreenshots => 1,
        ColorStrategy::Grayscale => 2,
        ColorStrategy::Random => 3,
    }
}

/// Order of the "Stil" dropdown: the modern styles first, the legacy
/// wave generator last.
pub(crate) const GENERATOR_STYLES: [GeneratorStyle; 7] = [
    GeneratorStyle::Layers,
    GeneratorStyle::Arcs,
    GeneratorStyle::Ribbons,
    GeneratorStyle::Planes,
    GeneratorStyle::Lines,
    GeneratorStyle::Mist,
    GeneratorStyle::Waves,
];

pub(crate) fn generator_style_for_index(index: u32) -> GeneratorStyle {
    GENERATOR_STYLES.get(index as usize).copied().unwrap_or(GeneratorStyle::Layers)
}

pub(crate) fn index_for_generator_style(style: GeneratorStyle) -> u32 {
    GENERATOR_STYLES.iter().position(|&s| s == style).unwrap_or(0) as u32
}

pub(crate) fn mood_for_index(index: u32) -> Mood {
    match index {
        1 => Mood::Light,
        2 => Mood::Dark,
        _ => Mood::Vivid,
    }
}

pub(crate) fn index_for_mood(mood: Mood) -> u32 {
    match mood {
        Mood::Vivid => 0,
        Mood::Light => 1,
        Mood::Dark => 2,
    }
}

/// Reflects a `GeneratedBackground`'s parameters onto the generator
/// controls — used both by `sync_background_controls`'s `Generated` arm
/// and after a fresh "Generieren" click updates the seed, mirroring
/// `sync_label_controls`'s role for the selected screenshot's label.
pub(crate) fn sync_generator_controls(window: &Window, generated: &GeneratedBackground) {
    window.generator_color_strategy_row().set_selected(index_for_color_strategy(generated.color_strategy));
    window.generator_style_row().set_selected(index_for_generator_style(generated.style));
    window.generator_mood_row().set_selected(index_for_mood(generated.mood));
    window.generator_grain_row().set_value(generated.grain * 100.0);
    let manual_buttons =
        [window.generator_manual_color_button_1(), window.generator_manual_color_button_2(), window.generator_manual_color_button_3(), window.generator_manual_color_button_4()];
    for (i, button) in manual_buttons.iter().enumerate() {
        let color = generated.palette.get(i).copied().unwrap_or(Rgba::new(0.5, 0.5, 0.5, 1.0));
        button.set_rgba(&gdk_rgba_from(&color));
    }
    window.generator_adapt_row().set_active(generated.adapt_to_screenshots);
    window.generator_inverse_contrast_row().set_value(generated.inverse_contrast * 100.0);
    window.generator_corner_bias_row().set_value(generated.corner_bias * 100.0);
    window.generator_scale_row().set_value(generated.scale * 100.0);
    window.generator_contrast_row().set_value(generated.contrast * 100.0);
    window.generator_seed_row().set_value(generated.seed as f64);
}

/// Reflects a `Background` value onto the type/color1/color2/angle controls
/// (used for both the initial sync and after undo/redo/load).
pub(crate) fn sync_background_controls(window: &Window, background: &Background) {

    match background {
        Background::Solid(color) => {
            window.background_type_row().set_selected(0);
            window.background_color_button().set_rgba(&gdk_rgba_from(color));
        }
        Background::Gradient(spec) => {
            let is_radial = matches!(spec.kind, GradientKind::Radial { .. });
            window.background_type_row().set_selected(if is_radial { 2 } else { 1 });
            if let Some((_, color)) = spec.stops.first() {
                window.background_color_button().set_rgba(&gdk_rgba_from(color));
            }
            if let Some((_, color)) = spec.stops.get(1) {
                window.gradient_color2_button().set_rgba(&gdk_rgba_from(color));
            }
            if let GradientKind::Linear { angle_deg } = spec.kind {
                window.gradient_angle_row().set_value(angle_deg);
            }
        }
        Background::Image(spec) => {
            window.background_type_row().set_selected(3);
            window.background_image_row().set_subtitle(&background_image_subtitle(&spec.source));
            window.background_image_fit_row().set_selected(index_for_background_image_fit(spec.fit));
            window.background_image_opacity_row().set_value(spec.opacity * 100.0);
        }
        Background::Generated(generated) => {
            window.background_type_row().set_selected(4);
            sync_generator_controls(window, generated);
        }
        Background::BlurredScreenshot(spec) => {
            window.background_type_row().set_selected(5);
            window.blurred_blur_row().set_value(spec.blur * 100.0);
            window.blurred_brightness_row().set_value(spec.brightness * 100.0);
        }
    }
}

/// Display text for the "Bilddatei" row's subtitle: the file name, or a
/// placeholder if the source isn't (as always today) a plain path.
pub(crate) fn background_image_subtitle(source: &ImageSource) -> String {
    match source {
        ImageSource::Path(path) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        ImageSource::Embedded { filename, .. } => filename.clone(),
    }
}

pub(crate) fn background_image_fit_for_index(index: u32) -> BackgroundImageFit {
    match index {
        0 => BackgroundImageFit::Cover,
        1 => BackgroundImageFit::Contain,
        2 => BackgroundImageFit::Fill,
        _ => BackgroundImageFit::Tile,
    }
}

pub(crate) fn index_for_background_image_fit(fit: BackgroundImageFit) -> u32 {
    match fit {
        BackgroundImageFit::Cover => 0,
        BackgroundImageFit::Contain => 1,
        BackgroundImageFit::Fill => 2,
        BackgroundImageFit::Tile => 3,
    }
}

/// Reads the background controls (type/color1/color2/angle) and builds the
/// `Background` they currently describe.
pub(crate) fn background_from_controls(window: &Window) -> Background {
    let color1 = rgba_from_gdk(&window.background_color_button().rgba());
    match window.background_type_row().selected() {
        1 => {
            let color2 = rgba_from_gdk(&window.gradient_color2_button().rgba());
            let angle_deg = window.gradient_angle_row().value();
            Background::Gradient(GradientSpec { kind: GradientKind::Linear { angle_deg }, stops: vec![(0.0, color1), (1.0, color2)] })
        }
        2 => {
            let color2 = rgba_from_gdk(&window.gradient_color2_button().rgba());
            // No manual center control yet — radial gradients are centered
            // on the composition (spec §8 leaves per-element/background
            // positioning controls for later).
            Background::Gradient(GradientSpec {
                kind: GradientKind::Radial { center_x: 0.5, center_y: 0.5 },
                stops: vec![(0.0, color1), (1.0, color2)],
            })
        }
        5 => Background::BlurredScreenshot(BlurredScreenshotSpec {
            blur: window.blurred_blur_row().value() / 100.0,
            brightness: window.blurred_brightness_row().value() / 100.0,
        }),
        _ => Background::Solid(color1),
    }
}

/// Applies whatever the background controls currently describe as one
/// undoable `SetBackground`, skipping the push if it doesn't actually
/// change anything — needed because `sync_controls_from_document` (after
/// undo/redo/load) sets these same controls to match the document it just
/// applied, which would otherwise re-fire this handler and wipe the redo
/// history it was trying to restore.
pub(crate) fn apply_background_from_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let mut state_ref = state.borrow_mut();
    let new = background_from_controls(window);
    if state_ref.syncing_controls || state_ref.document.background == new {
        return;
    }
    let old = state_ref.document.background.clone();
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(SetBackground { old, new }), document);
    drop(state_ref);
    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

/// Wires the canvas's own Alt-held wallpaper-drag gesture to an undoable
/// `SetBackground` — mirrors `register_label_drag`, but for a generated
/// background's own focus point (`offset_x`/`offset_y`) rather than a
/// screenshot's label. Never fires unless `doc.background` is already
/// `Background::Generated` at drag time (see `Canvas::connect_wallpaper_move`).
pub(crate) fn register_wallpaper_drag(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    canvas.connect_wallpaper_move(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |new_offset_x, new_offset_y| {
            let mut state_ref = state.borrow_mut();
            let Background::Generated(current) = &state_ref.document.background else { return };
            let mut new = current.clone();
            new.offset_x = new_offset_x;
            new.offset_y = new_offset_y;
            let old = state_ref.document.background.clone();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetBackground { old, new: Background::Generated(new) }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

/// Wires the "Bild" background's file picker, fit mode and opacity
/// controls (spec §8). Picking a file is the only action that actually
/// turns the background into `Background::Image` — selecting "Bild" in the
/// type row alone just reveals these controls, since there's nothing to
/// render without a file yet.
pub(crate) fn register_background_image_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let button = window.background_image_button();
    let fit_row = window.background_image_fit_row();
    let opacity_row = window.background_image_opacity_row();

    button.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| {
            let window = window.clone();
            let canvas = canvas.clone();
            let state = state.clone();
            glib::spawn_future_local(async move {
                let filter = gtk4::FileFilter::new();
                filter.add_mime_type("image/png");
                filter.add_mime_type("image/jpeg");
                filter.add_mime_type("image/webp");
                filter.set_name(Some("Bilder"));

                let dialog =
                    gtk4::FileDialog::builder().title("Hintergrundbild wählen").accept_label("Wählen").default_filter(&filter).build();

                let file = match dialog.open_future(Some(&window)).await {
                    Ok(file) => file,
                    Err(err) => {
                        if !err.matches(gtk4::DialogError::Dismissed) {
                            eprintln!("ScreenForge: background image dialog failed: {err}");
                        }
                        return;
                    }
                };
                let Some(path) = file.path() else { return };

                let mut state_ref = state.borrow_mut();
                if get_or_decode(&mut state_ref.image_cache, &path).is_none() {
                    return;
                }
                let fit = background_image_fit_for_index(window.background_image_fit_row().selected());
                let opacity = window.background_image_opacity_row().value() / 100.0;
                let old = state_ref.document.background.clone();
                let new = Background::Image(ImageBackgroundSpec { source: ImageSource::Path(path.clone()), fit, opacity });
                let EditorState { document, undo_stack, .. } = &mut *state_ref;
                undo_stack.apply(Box::new(SetBackground { old, new }), document);
                drop(state_ref);
                window.background_image_row().set_subtitle(&background_image_subtitle(&ImageSource::Path(path)));
                refresh_canvas(&window, &canvas, &state);
                update_undo_redo_sensitivity(&window, &state);
            });
        }
    ));

    fit_row.connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let mut state_ref = state.borrow_mut();
            let Background::Image(spec) = &state_ref.document.background else { return };
            let new_fit = background_image_fit_for_index(row.selected());
            if state_ref.syncing_controls || spec.fit == new_fit {
                return;
            }
            let old = state_ref.document.background.clone();
            let mut new_spec = spec.clone();
            new_spec.fit = new_fit;
            let new = Background::Image(new_spec);
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetBackground { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));

    opacity_row.connect_value_notify(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |row| {
            let mut state_ref = state.borrow_mut();
            let Background::Image(spec) = &state_ref.document.background else { return };
            let new_opacity = row.value() / 100.0;
            if state_ref.syncing_controls || (spec.opacity - new_opacity).abs() < f64::EPSILON {
                return;
            }
            let old = state_ref.document.background.clone();
            let mut new_spec = spec.clone();
            new_spec.opacity = new_opacity;
            let new = Background::Image(new_spec);
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetBackground { old, new }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    ));
}

/// Wires the "Automatische Farben" gradient button (spec: "Generate from
/// screenshots" / "Regenerate" — one button doing both, since a repeat
/// click naturally reads as "try another one").
pub(crate) fn register_gradient_auto_colors_control(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    window.gradient_generate_button().connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| generate_gradient_from_screenshots(&window, &canvas, &state)
    ));
}

/// Analyzes every currently visible screenshot's decoded pixels and
/// replaces the background with a suggested complementary gradient (spec
/// §3), preserving whichever of Linear/Radial the user currently has
/// selected. A no-op if nothing is imported yet — there's nothing to
/// analyze, and generating a plausible palette from zero screenshots would
/// just be an arbitrary color.
pub(crate) fn generate_gradient_from_screenshots(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let mut state_ref = state.borrow_mut();

    // Decode-on-demand into `image_cache` (self-healing/shared with every
    // other consumer — see its doc comment), then collect owned handles
    // (cheap: `DecodedImage` wraps an `Arc<[u8]>`) so the borrow of
    // `image_cache` ends before `PixelSample`s borrow from them below.
    let paths: Vec<PathBuf> = state_ref
        .document
        .elements
        .iter()
        .filter(|e| e.visible)
        .filter_map(|e| match &e.source {
            ImageSource::Path(path) => Some(path.clone()),
            ImageSource::Embedded { .. } => None,
        })
        .collect();
    let images: Vec<DecodedImage> =
        paths.iter().filter_map(|path| get_or_decode(&mut state_ref.image_cache, path).cloned()).collect();
    if images.is_empty() {
        return;
    }

    let seed = state_ref.gradient_auto_seed;
    state_ref.gradient_auto_seed = seed.wrapping_add(1);

    let samples: Vec<screenforge_core::palette::PixelSample> =
        images.iter().map(|image| screenforge_core::palette::PixelSample { bytes: &image.bytes, width: image.width, height: image.height }).collect();
    let mut spec = screenforge_core::palette::suggest_gradient(&samples, seed);
    // A suggestion is always computed as a linear gradient (an angle only
    // means something for Linear) -- if the user has Radial selected,
    // keep the same two suggested colors but center them, rather than
    // silently switching their chosen gradient kind back to Linear.
    if window.background_type_row().selected() == 2 {
        spec.kind = GradientKind::Radial { center_x: 0.5, center_y: 0.5 };
    }
    let new = Background::Gradient(spec);

    let old = state_ref.document.background.clone();
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(SetBackground { old, new: new.clone() }), document);
    drop(state_ref);

    // The generated colors didn't come from the color/angle controls (the
    // usual source of truth for `apply_background_from_controls`), so
    // those controls need to be told what actually landed, the same way
    // undo/redo/project-load does via `sync_background_controls` --
    // guarded the same way, since e.g. `set_rgba` below would otherwise
    // reentrantly fire `apply_background_from_controls` for each control.
    state.borrow_mut().syncing_controls = true;
    sync_background_controls(window, &new);
    state.borrow_mut().syncing_controls = false;

    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
}

/// Wires every generator parameter control (style, color strategy, adapt-
/// to-screenshots, the eight sliders, and directly editing the seed row)
/// straight onto the current `GeneratedBackground` — live and undoable,
/// but never re-resolving the palette or picking a new seed on its own;
/// only `generate_background` (the "Generieren" button, or first
/// switching the background type to "Generiert") does that. This mirrors
/// `apply_title`/`apply_shadow_geometry`'s split elsewhere: cheap
/// parameter edits stay separate from the one action that's meant to
/// visibly reroll the composition.
pub(crate) fn register_generator_controls(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let apply = glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move || {
            let mut state_ref = state.borrow_mut();
            if state_ref.syncing_controls {
                return;
            }
            let Background::Generated(current) = state_ref.document.background.clone() else { return };
            let color_strategy = color_strategy_for_index(window.generator_color_strategy_row().selected());
            let palette = if matches!(color_strategy, ColorStrategy::Manual) {
                [
                    window.generator_manual_color_button_1().rgba(),
                    window.generator_manual_color_button_2().rgba(),
                    window.generator_manual_color_button_3().rgba(),
                    window.generator_manual_color_button_4().rgba(),
                ]
                .iter()
                .map(rgba_from_gdk)
                .collect()
            } else {
                current.palette.clone()
            };
            let style = generator_style_for_index(window.generator_style_row().selected());
            let mood = mood_for_index(window.generator_mood_row().selected());
            let seed = window.generator_seed_row().value() as u64;
            // Style and mood decide how a derived palette is resolved (see
            // `resolve_palette_for`), so switching either one re-resolves
            // it for the current seed rather than waiting for the next
            // "Generieren" click — otherwise the mood dropdown would look
            // like it does nothing.
            let palette = if !matches!(color_strategy, ColorStrategy::Manual) && (style != current.style || mood != current.mood) {
                let inverse_contrast = window.generator_inverse_contrast_row().value() / 100.0;
                resolve_generator_palette(&mut state_ref, color_strategy, inverse_contrast, seed, style, mood)
            } else {
                palette
            };
            let new = GeneratedBackground {
                seed,
                style,
                mood,
                grain: window.generator_grain_row().value() / 100.0,
                color_strategy,
                palette,
                adapt_to_screenshots: window.generator_adapt_row().is_active(),
                inverse_contrast: window.generator_inverse_contrast_row().value() / 100.0,
                corner_bias: window.generator_corner_bias_row().value() / 100.0,
                scale: window.generator_scale_row().value() / 100.0,
                // No sliders for these — `offset_x`/`offset_y` are only
                // ever changed by dragging the wallpaper directly on the
                // canvas (see `register_wallpaper_drag`), and the rest are
                // only ever redrawn from scratch by `generate_background`'s
                // own randomization — so editing any *other* generator
                // control must leave all of them exactly as they were.
                offset_x: current.offset_x,
                offset_y: current.offset_y,
                density: current.density,
                flow: current.flow,
                variation: current.variation,
                contrast: window.generator_contrast_row().value() / 100.0,
                softness: current.softness,
            };
            if current == new {
                return;
            }
            let old = state_ref.document.background.clone();
            let EditorState { document, undo_stack, .. } = &mut *state_ref;
            undo_stack.apply(Box::new(SetBackground { old, new: Background::Generated(new) }), document);
            drop(state_ref);
            refresh_canvas(&window, &canvas, &state);
            update_undo_redo_sensitivity(&window, &state);
        }
    );

    window.generator_color_strategy_row().connect_selected_notify(glib::clone!(
        #[weak]
        window,
        #[strong]
        state,
        #[strong]
        apply,
        move |row| {
            let new_strategy = color_strategy_for_index(row.selected());
            // Entering Manual mode for the first time in this background
            // (not just re-visiting it) starts from a curated, genuinely
            // contrasting palette rather than whatever the *previous*
            // strategy happened to leave in the 4 buttons — spec: "die
            // vier Felder [enthielten] lediglich unterschiedliche
            // Rotwerte", which is exactly what inheriting e.g. a
            // `FromScreenshots` palette (same hue, only lightness varies)
            // produced. Guarded by `syncing_controls` so setting all 4
            // buttons doesn't fire `apply()` four times over.
            if matches!(new_strategy, ColorStrategy::Manual) {
                let was_already_manual =
                    matches!(&state.borrow().document.background, Background::Generated(g) if matches!(g.color_strategy, ColorStrategy::Manual));
                if !was_already_manual {
                    let was_syncing = state.borrow().syncing_controls;
                    state.borrow_mut().syncing_controls = true;
                    let buttons = [
                        window.generator_manual_color_button_1(),
                        window.generator_manual_color_button_2(),
                        window.generator_manual_color_button_3(),
                        window.generator_manual_color_button_4(),
                    ];
                    for (button, color) in buttons.iter().zip(screenforge_core::palette::DEFAULT_MANUAL_PALETTE.iter()) {
                        button.set_rgba(&gdk_rgba_from(color));
                    }
                    state.borrow_mut().syncing_controls = was_syncing;
                }
            }
            apply();
        }
    ));
    window.generator_manual_color_button_1().connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_manual_color_button_2().connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_manual_color_button_3().connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_manual_color_button_4().connect_rgba_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_adapt_row().connect_active_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_inverse_contrast_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_corner_bias_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_scale_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_contrast_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_seed_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_style_row().connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_mood_row().connect_selected_notify(glib::clone!(#[strong] apply, move |_| apply()));
    window.generator_grain_row().connect_value_notify(glib::clone!(#[strong] apply, move |_| apply()));

    window.generator_generate_row().connect_activated(glib::clone!(
        #[weak]
        window,
        #[weak]
        canvas,
        #[strong]
        state,
        move |_| generate_background(&window, &canvas, &state)
    ));
}

/// Resolves a generator palette from the currently visible screenshots'
/// pixels (decoded through the shared image cache) for `strategy`, `style`
/// and `mood`. Not for `ColorStrategy::Manual`, whose palette is the user's
/// own colors.
pub(crate) fn resolve_generator_palette(
    state: &mut EditorState,
    strategy: ColorStrategy,
    inverse_contrast: f64,
    seed: u64,
    style: GeneratorStyle,
    mood: Mood,
) -> Vec<Rgba> {
    let paths: Vec<PathBuf> = state
        .document
        .elements
        .iter()
        .filter(|e| e.visible)
        .filter_map(|e| match &e.source {
            ImageSource::Path(path) => Some(path.clone()),
            ImageSource::Embedded { .. } => None,
        })
        .collect();
    let images: Vec<DecodedImage> = paths.iter().filter_map(|path| get_or_decode(&mut state.image_cache, path).cloned()).collect();
    let samples: Vec<screenforge_core::palette::PixelSample> = images
        .iter()
        .map(|image| screenforge_core::palette::PixelSample { bytes: &image.bytes, width: image.width, height: image.height })
        .collect();
    screenforge_core::palette::resolve_palette_for(&samples, strategy, inverse_contrast, seed, style, mood)
}

/// Resolves a fresh palette (from the currently visible screenshots, per
/// whatever color strategy is selected) and picks a new seed, then commits
/// a `Background::Generated` built from the current controls — the
/// "Generieren"/"Regenerate" action (spec: one button doing both, a repeat
/// click reading naturally as "try another one", same as the earlier
/// gradient auto-colors button). Also what switching the background type
/// to "Generiert" calls, since there's nothing to render yet at that point
/// either.
pub(crate) fn generate_background(window: &Window, canvas: &Canvas, state: &Rc<RefCell<EditorState>>) {
    let mut state_ref = state.borrow_mut();
    if state_ref.syncing_controls {
        // Reached via `background_type_row`'s own notify handler, which
        // `sync_controls_from_document`'s `background_type_row.set_selected(5)`
        // fires reentrantly while restoring a saved/undone `Generated`
        // background. Regenerating here would discard the very seed and
        // palette that call was trying to restore — exactly the
        // reproducibility guarantee this whole feature exists for.
        return;
    }

    let previous_seed = match &state_ref.document.background {
        Background::Generated(g) => g.seed,
        _ => 0,
    };
    // A "Generieren"/"Regenerate" click rerolls the pattern itself, not
    // where the user has already dragged its focus point to — carried
    // over unchanged, the same way `sync_generator_controls` never touches
    // it either.
    let (previous_offset_x, previous_offset_y) = match &state_ref.document.background {
        Background::Generated(g) => (g.offset_x, g.offset_y),
        _ => (0.0, 0.0),
    };
    // A fresh seed each click, derived deterministically from the last one
    // (plus a fixed salt) through the same `Rng` generation itself uses —
    // this needs no external randomness source, and still gives a
    // different-looking result practically every time.
    let mut seed_source = screenforge_core::rng::Rng::new(previous_seed ^ 0x5EED_5EED_5EED_5EED);
    let new_seed = seed_source.next_u64() % 1_000_000_000;
    // Density/flow/variation/softness aren't exposed as sliders — spec:
    // "immer wieder neu Zufallswerte erzeugen" (always generate fresh random
    // values) rather than have the user tune them by hand. Drawing them from
    // the same `seed_source` keeps a "Generieren" click's whole result
    // (seed *and* these) deterministic from `previous_seed`, matching every
    // other value derived here.
    let density = seed_source.range(0.0, 1.0);
    let flow = seed_source.range(0.0, 1.0);
    let variation = seed_source.range(0.0, 1.0);
    let softness = seed_source.range(0.0, 1.0);

    let color_strategy = color_strategy_for_index(window.generator_color_strategy_row().selected());
    let inverse_contrast = window.generator_inverse_contrast_row().value() / 100.0;
    let style = generator_style_for_index(window.generator_style_row().selected());
    let mood = mood_for_index(window.generator_mood_row().selected());
    let palette = if matches!(color_strategy, ColorStrategy::Manual) {
        [
            window.generator_manual_color_button_1().rgba(),
            window.generator_manual_color_button_2().rgba(),
            window.generator_manual_color_button_3().rgba(),
            window.generator_manual_color_button_4().rgba(),
        ]
        .iter()
        .map(rgba_from_gdk)
        .collect()
    } else {
        resolve_generator_palette(&mut state_ref, color_strategy, inverse_contrast, new_seed, style, mood)
    };

    let new = GeneratedBackground {
        seed: new_seed,
        style,
        mood,
        grain: window.generator_grain_row().value() / 100.0,
        color_strategy,
        palette,
        adapt_to_screenshots: window.generator_adapt_row().is_active(),
        inverse_contrast,
        corner_bias: window.generator_corner_bias_row().value() / 100.0,
        offset_x: previous_offset_x,
        offset_y: previous_offset_y,
        scale: window.generator_scale_row().value() / 100.0,
        density,
        flow,
        variation,
        contrast: window.generator_contrast_row().value() / 100.0,
        softness,
    };

    let old = state_ref.document.background.clone();
    let EditorState { document, undo_stack, .. } = &mut *state_ref;
    undo_stack.apply(Box::new(SetBackground { old, new: Background::Generated(new.clone()) }), document);
    drop(state_ref);

    // The seed just changed; reflect it (and the freshly resolved
    // strategy-driven visibility) back onto the controls the same guarded
    // way `generate_gradient_from_screenshots` does.
    state.borrow_mut().syncing_controls = true;
    sync_generator_controls(window, &new);
    state.borrow_mut().syncing_controls = false;

    refresh_canvas(window, canvas, state);
    update_undo_redo_sensitivity(window, state);
    rebuild_sidebar_variants(window, canvas, state);
}
