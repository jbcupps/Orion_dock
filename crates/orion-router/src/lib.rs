//! Id/Ego cognitive router for the Orion runtime.
//!
//! Routes user messages between the local LLM (Id) and an optional cloud LLM
//! (Ego). Supports configurable [`RoutingMode`] (Id-primary or Ego-primary),
//! optional Superego safety pre-checks, and provider auto-detection from API
//! key prefixes.

pub mod router;

pub use router::{EgoProvider, IdEgoRouter, RoutingMode, SuperegoResult};
