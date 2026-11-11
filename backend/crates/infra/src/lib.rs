//! Technical adapters: configuration, process signals, PostgreSQL, and later
//! S3 storage, email and Valkey.

pub mod accounts;
pub mod auth_policy;
pub mod bootstrap;
pub mod config;
pub mod credentials;
pub mod crypto;
pub mod db;
pub mod email;
pub mod groups;
pub mod health;
pub mod http_client;
pub mod id;
pub mod idempotency;
pub mod invitations;
pub mod jobs;
pub mod members;
pub mod memberships;
pub mod mfa;
pub mod oidc;
pub mod oidc_providers;
pub mod pagination;
pub mod platform;
pub mod rate_limit;
pub mod reference_items;
pub mod sessions;
pub mod shutdown;
pub mod telemetry;
pub mod tenant_switch;
pub mod valkey;

pub use sqlx;
pub use uuid;
