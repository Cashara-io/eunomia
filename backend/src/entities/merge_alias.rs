//! Kind-qualified redirects from explicit owner merges, stored in the existing aliases array.
//! Reserve the namespace at ordinary write boundaries so extraction cannot invent a redirect.

pub(super) const PREFIX: &str = "@eunomia/merged/";

pub(super) fn encode(kind: &str, name: &str) -> String {
    format!("{PREFIX}{kind}:{}", name.trim().to_lowercase())
}

pub(super) fn decode(alias: &str) -> Option<(&str, &str)> {
    let (kind, name) = alias.strip_prefix(PREFIX)?.split_once(':')?;
    super::service::KINDS.contains(&kind).then_some((kind, name))
}

pub(super) fn reserved(name: &str) -> bool {
    name.trim().to_lowercase().starts_with(PREFIX)
}

pub(super) fn visible(aliases: &[String]) -> Vec<String> {
    aliases.iter().filter(|a| !reserved(a)).cloned().collect()
}

pub(super) fn validate(name: Option<&str>, aliases: &[String]) -> crate::error::AppResult<()> {
    if name.is_some_and(reserved) || aliases.iter().any(|a| reserved(a)) {
        return Err(crate::error::AppError::bad_request("entity names and aliases use a reserved merge namespace"));
    }
    Ok(())
}
