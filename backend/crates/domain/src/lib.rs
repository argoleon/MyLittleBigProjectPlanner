//! Business rules: capacity, workload, recurrences, workflows, query language.
//!
//! This crate performs no I/O and depends on no other crate, so every rule
//! can be unit-tested in isolation. `just check-domain` enforces it.

pub mod account;
pub mod aggregates;
pub mod auth_policy;
pub mod configuration;
pub mod devices;
pub mod erasure;
pub mod grants;
pub mod hierarchy;
pub mod i18n;
pub mod idempotency;
pub mod issue_types;
pub mod jobs;
pub mod members;
pub mod mfa;
pub mod oidc;
pub mod permissions;
pub mod reference_item;
pub mod tenancy;
pub mod tenant;
pub mod validation;

/// Name of the product, shared by every service.
pub const PRODUCT_NAME: &str = "My Little Big Project Planner";
