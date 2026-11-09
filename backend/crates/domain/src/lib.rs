//! Business rules: capacity, workload, recurrences, workflows, query language.
//!
//! This crate performs no I/O and depends on no other crate, so every rule
//! can be unit-tested in isolation. `just check-domain` enforces it.

pub mod tenancy;

/// Name of the product, shared by every service.
pub const PRODUCT_NAME: &str = "My Little Big Project Planner";
