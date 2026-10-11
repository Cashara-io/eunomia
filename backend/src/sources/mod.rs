//! Pluggable data sources: one module per provider (fetch + map), plus the
//! registry that runs them through the ingest pipeline and the scheduler
//! that decides what is due. See `docs/connectors.md` for the user-facing list.

pub mod base;
pub mod demo;
pub mod discord;
pub mod github;
pub mod gmail;
pub mod google_calendar;
pub mod heypocket;
pub mod linear;
#[cfg(any(test, feature = "test-support"))]
pub mod mock;
pub mod notion;
pub mod registry;
pub mod routing;
pub mod scheduler;
pub mod slack;
pub mod spotify;
pub mod stripe;
pub mod todoist;
pub mod up_bank;
