//! Readers for Substance file formats.
//!
//! `.sbs` (Designer source graphs, XML) → [`reetched_graph::Package`].
//! The XML layout here was written from what `.sbs` files are publicly known
//! to contain, without Adobe's SDK; it is verified only against files we
//! authored ourselves (see the tests).
//!
//! [`bridge`]: Painter projects exported by the ReEtched Bridge plugin
//! (`tools/painter-bridge`) → documents.

pub mod bridge;
pub mod sbs;
