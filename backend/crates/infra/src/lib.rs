//! Technical adapters: configuration, process signals, PostgreSQL, and later
//! S3 storage, email and Valkey.

pub mod config;
pub mod db;
pub mod health;
pub mod id;
pub mod idempotency;
pub mod pagination;
pub mod reference_items;
pub mod shutdown;
pub mod telemetry;

pub use sqlx;
pub use uuid;
