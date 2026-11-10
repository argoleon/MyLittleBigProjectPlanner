//! Technical adapters: configuration, process signals, PostgreSQL, and later
//! S3 storage, email and Valkey.

pub mod accounts;
pub mod config;
pub mod credentials;
pub mod db;
pub mod email;
pub mod health;
pub mod id;
pub mod idempotency;
pub mod jobs;
pub mod pagination;
pub mod rate_limit;
pub mod reference_items;
pub mod sessions;
pub mod shutdown;
pub mod telemetry;
pub mod valkey;

pub use sqlx;
pub use uuid;
