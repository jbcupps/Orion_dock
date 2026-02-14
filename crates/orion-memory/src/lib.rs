//! Persistent memory store for Orion agents.
//!
//! Supports SQLite (default, file-backed) and PostgreSQL (feature-gated via
//! `postgres`). Memories are weighted as [`Ephemeral`](MemoryWeight::Ephemeral),
//! [`Distilled`](MemoryWeight::Distilled), or
//! [`Crystallized`](MemoryWeight::Crystallized), and include birth records,
//! operational chat history, and general agent memories.

#[cfg(feature = "postgres")]
pub(crate) mod postgres_store;
pub mod schema;
pub(crate) mod sqlite_store;
pub mod store;

pub use schema::*;
pub use store::{Memory, MemoryStore, MemoryWeight, Result as MemoryResult, StoreError};
