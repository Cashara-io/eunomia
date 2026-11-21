//! Read Cupel's own documentation.
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{register, bad_args, ToolSpec};

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    #[derive(serde::Deserialize, Default)]
    struct DocsArgs {
        #[serde(default)]
        topic: Option<String>,
    }
    register(
        registry,
        "docs",
        json!({
            "type": "object",
            "properties": {
                "topic": {"type": "string", "description": "quickstart, installation, agents, concepts or deployment; omit to list"},
            },
        }),
        Arc::new(|_state, _owner, args| {
            Box::pin(async move {
                let a: DocsArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let list = || crate::docs::DOCS.iter().map(|d| json!({ "topic": d.slug, "title": d.title })).collect::<Vec<_>>();
                Ok(match a.topic {
                    None => json!({ "docs": list() }),
                    Some(t) => match crate::docs::find(&t) {
                        Some(d) => json!({ "topic": d.slug, "title": d.title, "markdown": d.body }),
                        None => json!({ "error": format!("no doc called {t:?}"), "docs": list() }),
                    },
                })
            })
        }),
    );
}
