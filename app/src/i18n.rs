//! Translations via gettext. Source strings are English; `po/` holds the
//! translations (German first). An installed build finds its catalogs in
//! `$prefix/share/locale` (`SCREENFORGE_LOCALEDIR`, set by meson at
//! compile time); `cargo run` uses the catalogs `build.rs` compiles into
//! `OUT_DIR`.

use gettextrs::{bind_textdomain_codeset, bindtextdomain, setlocale, textdomain, LocaleCategory};

pub(crate) const GETTEXT_PACKAGE: &str = "screenforge";

fn locale_dir() -> &'static str {
    option_env!("SCREENFORGE_LOCALEDIR").unwrap_or(concat!(env!("OUT_DIR"), "/locale"))
}

pub(crate) fn init() {
    // SAFETY: called once at startup, before any other thread exists.
    unsafe {
        setlocale(LocaleCategory::LcAll, "");
    }
    if let Err(err) = bindtextdomain(GETTEXT_PACKAGE, locale_dir()) {
        eprintln!("ScreenForge: could not bind translations: {err}");
    }
    let _ = bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8");
    let _ = textdomain(GETTEXT_PACKAGE);
}

/// Marks a string for extraction (`xgettext --keyword=N_`) where a call to
/// `gettext` isn't possible, e.g. in a `const` table; translate it with
/// `gettext` where it is shown.
#[allow(non_snake_case)]
pub(crate) const fn N_(s: &'static str) -> &'static str {
    s
}
