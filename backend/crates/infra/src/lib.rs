//! Technical adapters: configuration, process signals, PostgreSQL, and later
//! S3 storage, email and Valkey.

pub mod accounts;
pub mod aggregates;
pub mod auth_policy;
pub mod bootstrap;
pub mod config;
pub mod configs;
pub mod credentials;
pub mod crypto;
pub mod db;
pub mod email;
pub mod erasure;
pub mod exports;
pub mod grants;
pub mod groups;
pub mod health;
pub mod http_client;
pub mod id;
pub mod idempotency;
pub mod invitations;
pub mod issue_types;
pub mod jobs;
pub mod jwt_crypto;
pub mod members;
pub mod memberships;
pub mod mfa;
pub mod nodes;
pub mod oidc;
pub mod oidc_providers;
pub mod pagination;
pub mod permissions;
pub mod platform;
pub mod rate_limit;
pub mod reference_items;
pub mod roles;
pub mod sessions;
pub mod shutdown;
pub mod telemetry;
pub mod tenant_switch;
pub mod valkey;
pub mod workflows;

pub use sqlx;
pub use uuid;
