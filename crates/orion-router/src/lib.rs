//! Id/Ego cognitive router for the Orion runtime.
//!
//! Routes user messages between the local LLM (Id) and an optional cloud LLM
//! (Ego). Supports configurable [`RoutingMode`] (Id-primary or Ego-primary),
//! optional Superego safety pre-checks, and provider auto-detection from API
//! key prefixes.
//!
//! Also provides the Execution Governor architecture: structured planning via
//! GoalFrame, governed execution loops with loop detection, and cognitive
//! transform traits for composable agent pipelines.

pub mod cognitive;
pub mod constraint_store;
pub mod council;
pub mod execution_state;
pub mod goal_frame;
pub mod governor;
pub mod planner;
pub mod router;

pub use router::{
    build_ego_provider, EgoProvider, IdEgoRouter, RoutingMode, SuperegoL2Mode, SuperegoResult,
};
