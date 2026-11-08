//! The user docs in `/docs`, compiled into the binary so the app's Docs page
//! (`GET /api/docs`) and the `docs` MCP tool serve exactly what's in the repo.
//! In Docker the folder arrives as the `docs` build context (see Dockerfile).

pub struct Doc {
    pub slug: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

pub const DOCS: &[Doc] = &[
    Doc { slug: "quickstart", title: "Quickstart", body: include_str!("../../docs/quickstart.md") },
    Doc { slug: "installation", title: "Installation", body: include_str!("../../docs/installation.md") },
    Doc { slug: "agents", title: "Connecting AI apps", body: include_str!("../../docs/agents.md") },
    Doc { slug: "concepts", title: "How Eunomia works", body: include_str!("../../docs/concepts.md") },
    Doc { slug: "connectors", title: "Connectors", body: include_str!("../../docs/connectors.md") },
    Doc { slug: "documents", title: "Documents", body: include_str!("../../docs/documents.md") },
    Doc { slug: "deployment", title: "Deployment", body: include_str!("../../docs/deployment.md") },
];

/// The built-in memory skill (`docs/eunomia-skill.md`): SKILL.md frontmatter
/// and instructions telling an agent when to recall and what to remember. Users
/// can override it per account (Settings → Memory skill).
pub const DEFAULT_SKILL: &str = include_str!("../../docs/eunomia-skill.md");

/// The user's skill, or the built-in one when they haven't customised it.
pub fn effective_skill(custom: &str) -> &str {
    if custom.trim().is_empty() { DEFAULT_SKILL } else { custom }
}

/// Skill text without its `---` frontmatter block, for injecting as context.
pub fn skill_body(skill: &str) -> &str {
    let t = skill.trim_start();
    t.strip_prefix("---")
        .and_then(|rest| rest.split_once("\n---"))
        .map(|(_, body)| body.trim_start_matches(|c| c != '\n').trim_start())
        .unwrap_or(t)
}

pub fn find(slug: &str) -> Option<&'static Doc> {
    DOCS.iter().find(|d| d.slug.eq_ignore_ascii_case(slug.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_doc_has_a_title_heading_and_is_findable() {
        for d in DOCS {
            assert!(d.body.starts_with("# "), "{} should start with a heading", d.slug);
            assert_eq!(find(d.slug).unwrap().slug, d.slug);
        }
        assert!(find(" Quickstart ").is_some());
        assert!(find("nope").is_none());
    }

    #[test]
    fn skill_defaults_and_strips_frontmatter() {
        assert!(effective_skill("  ").starts_with("---\nname: eunomia-memory"));
        assert_eq!(effective_skill("# mine"), "# mine");
        assert!(skill_body(DEFAULT_SKILL).starts_with("# Eunomia memory"));
        assert_eq!(skill_body("no frontmatter"), "no frontmatter");
        assert_eq!(skill_body("---\nname: x\n---\n\nbody"), "body");
    }

    #[test]
    fn default_skill_only_names_real_tools() {
        let tools = crate::tools::registry::all_tools();
        for name in ["recall", "reflect", "docs", "memory_write", "memory_update", "memory_delete", "code_entity_upsert", "code_relate", "entity_relate", "vault_list"] {
            assert!(DEFAULT_SKILL.contains(&format!("`{name}`")), "skill no longer mentions `{name}`; update this list");
            assert!(tools.contains_key(name), "skill mentions unknown tool `{name}`");
        }
    }

    #[test]
    fn docs_cover_every_mcp_tool() {
        let agents = find("agents").unwrap().body;
        for name in crate::tools::registry::all_tools().keys() {
            assert!(agents.contains(&format!("`{name}`")), "docs/agents.md doesn't mention `{name}`");
        }
    }
}
