//! Indexes IFS Cloud Core and Cust source folders into one dependency graph.
//!
//! Pipeline: `scan` walks both roots → per-type parsers (`marble`, `plsql`,
//! `deployini`) → `build` groups files into logical units, resolves names and
//! creates edges → `analyze` produces findings (cycles, duplicates, layer and
//! deploy.ini problems).
//!
//! Layer is decided by which root a file lives in. A `-Cust` file whose
//! logical name also exists in Core is an *overlay*; one with no Core match is
//! a *pure Cust base*. `@Override` resolves to whichever base exists, Core or
//! pure Cust.

pub mod analyze;
pub mod build;
pub mod deployini;
pub mod graph;
pub mod json;
pub mod marble;
pub mod model;
pub mod plsql;
pub mod scan;

pub use build::{index, IndexOptions};
pub use model::*;
