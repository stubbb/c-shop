//! Serving the editor to a program, over a network.
//!
//! The script harness already lets something that cannot see a canvas drive
//! the editor. This puts that behind a socket and speaks the Model Context
//! Protocol over it, so the caller need not be on the same machine — and, more
//! to the point, so a tool result can carry a *picture*. That closes the loop
//! the harness was built for: describe, draw, look, correct.
//!
//! * [`http`] — enough HTTP/1.1 to carry it, hand-written
//! * JSON comes from [`cshop_core::json`], which is hand-written for the same
//!   reason and is shared with the vision pack
//! * [`protocol`] — JSON-RPC framing and the protocol's own methods
//! * [`tools`] — what a caller may actually ask for
//! * [`reference`] — the manual, for a caller that has not read one
//! * [`editor`] — the single thread that owns the GPU and the open documents
//! * [`server`] — binding, and who is allowed to talk

pub mod base64;
pub mod editor;
pub mod http;
pub mod protocol;
pub mod reference;
pub mod server;
pub mod tools;

/// JSON lives in the core crate, since more than the server needs it now.
pub use cshop_core::json;
