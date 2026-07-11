//! ghbox-core: GraphQL fetch, comment filtering, and read-state management
//! for "PRs where the ball is in my court" on GitHub.

pub mod config;
mod error;
pub mod filter;
pub mod store;
pub mod types;

pub use error::{Error, Result};
