//! The sidebar's panels, one module per topic.

pub(crate) mod background;
pub(crate) mod callouts;
pub(crate) mod effects;
pub(crate) mod export;
pub(crate) mod layout;
pub(crate) mod output;
pub(crate) mod redactions;
pub(crate) mod text;
pub(crate) mod variants;

pub(crate) use background::*;
pub(crate) use callouts::*;
pub(crate) use effects::*;
pub(crate) use export::*;
pub(crate) use layout::*;
pub(crate) use output::*;
pub(crate) use redactions::*;
pub(crate) use text::*;
pub(crate) use variants::*;
