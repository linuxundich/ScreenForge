//! screenforge-core: GTK-independent data model, layout engine, and
//! rendering. No GTK/GDK/Adwaita dependency is allowed in this crate — that
//! boundary is what keeps the model and layout engine unit-testable.

pub mod command;
pub mod frame;
pub mod generator;
pub mod layout;
pub mod library;
pub mod model;
pub mod palette;
pub mod project;
pub mod render;
pub mod shadow_cache;
pub mod snap;
pub mod warp;
pub mod template;

// Shared with Kulisse through Prospekt; re-exported under their old paths.
pub use prospekt::{background_cache, blur, rng};
