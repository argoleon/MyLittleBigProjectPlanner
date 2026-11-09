//! Technical adapters: configuration, process signals, PostgreSQL, and later
//! S3 storage, email and Valkey.

pub mod config;
pub mod db;
pub mod id;
pub mod shutdown;

pub use sqlx;
pub use uuid;
