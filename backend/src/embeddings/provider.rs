//! The provider's address and credential rules now live with the decision point (`crate::ai::provider`).
//! What decides nothing about a call stays reachable here; a request can only be made through `crate::ai`.

pub use crate::ai::provider::{effective_base_url, pick_chat_model, same_url, Provider, DEFAULT_CHAT_MODEL, OPENAI_BASE_URL};
