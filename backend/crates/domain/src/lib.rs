//! Business rules: capacity, workload, recurrences, workflows, query language.
//!
//! This crate performs no I/O and depends on no other crate, so every rule
//! can be unit-tested in isolation. `just check-domain` enforces it.

pub mod absences;
pub mod account;
pub mod aggregates;
pub mod assignments;
pub mod attachments;
pub mod audit;
pub mod auth_policy;
pub mod boards;
pub mod bulk;
pub mod calendars;
pub mod capacity;
pub mod comments;
pub mod configuration;
pub mod dates;
pub mod devices;
pub mod erasure;
pub mod export;
pub mod fields;
pub mod grants;
pub mod hierarchy;
pub mod holidays;
pub mod i18n;
pub mod idempotency;
pub mod issue_types;
pub mod issues;
pub mod jobs;
pub mod links;
pub mod members;
pub mod mfa;
pub mod migration;
pub mod notifications;
pub mod notify;
pub mod oidc;
pub mod permissions;
pub mod query;
pub mod reference_item;
pub mod resources;
pub mod rich_text;
pub mod saved_filters;
pub mod screens;
pub mod teams;
pub mod tenancy;
pub mod tenant;
pub mod text_search;
pub mod transitions;
pub mod validation;
pub mod workflows;

/// Name of the product, shared by every service.
pub const PRODUCT_NAME: &str = "My Little Big Project Planner";
