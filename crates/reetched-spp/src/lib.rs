//! Read-only conversion of Substance 3D Painter projects (`.spp`) into
//! ReEtched documents. Nothing here writes Painter files.

pub mod container;
pub mod convert;
pub mod document;
pub mod image;
pub mod qt;
pub mod strokes;
