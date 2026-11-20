//! The operations on tickets shared by the API and the worker (L06-US08):
//! what an edit, a transition or a move does once the member may do it and
//! the ticket is locked. Callers check who acts and the ticket's version;
//! these write the ticket, its history, its watchers and its outbox events.

pub mod comments;
pub mod content;
pub mod edit;
pub mod history;
pub mod moves;
pub mod transition;

pub use content::{
    Content, ContentError, field_defs, ids_in, read_description, read_fields, stored_values, type_ids_of,
    unknown_references,
};
