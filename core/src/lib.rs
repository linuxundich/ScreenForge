//! screenforge-core: GTK-independent data model, layout engine, and
//! rendering. No GTK/GDK/Adwaita dependency is allowed in this crate — that
//! boundary is what keeps the model and layout engine unit-testable.

pub mod command;
pub mod decoration;
pub mod layout;
pub mod model;
pub mod project;
pub mod render;
pub mod snap;
pub mod template;
