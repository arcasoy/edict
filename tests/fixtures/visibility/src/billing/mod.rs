pub(crate) mod tax; // leak: visible crate-wide
pub(super) mod ledger; // leak: `super` of billing is the crate root
pub mod api; // leak, unless a library opts in to nested public API
mod invoice; // fine

pub use invoice::Invoice;
