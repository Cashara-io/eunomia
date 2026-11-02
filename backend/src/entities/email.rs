//! Gmail-specific helpers for entity extraction: cleaning a body before it reaches the model, deciding
//! which emails are not worth a model call, and capping what the model may return. All pure, so they
//! are unit-tested without a database or network.

use serde_json::Value;

/// Facts kept per email, across all entities.
pub const MAX_EMAIL_FACTS: usize = 3;
/// Relation labels an email may produce.
pub const EMAIL_RELATION_LABELS: &[&str] = &["works_at", "studies_at", "teaches", "manages", "member_of", "located_in"];
/// How far back a same-sender, same-subject email counts as the earlier copy of a reminder.
pub const REPEAT_WINDOW_DAYS: i64 = 14;

/// Below this many characters (after the `From:` line) stripping is undone: the body was mostly quote.
const MIN_STRIPPED_LEN: usize = 40;
/// A `-- ` line only starts a signature when this few lines or fewer follow it.
const MAX_SIGNATURE_LINES: usize = 15;
/// A wrapped "On ... wrote:" attribution is never longer than this once joined.
const MAX_ATTRIBUTION_LEN: usize = 300;

fn is_attribution(line: &str, next: Option<&str>) -> bool {
    let t = line.trim();
    if !t.starts_with("On ") {
        return false;
    }
    if t.ends_with("wrote:") {
        return true;
    }
    // wrapped over two lines: "On Mon, 3 Mar 2025 at 10:00, Ada Lovelace <ada@x.org>" / "wrote:"
    next.is_some_and(|n| n.trim().ends_with("wrote:") && t.len() + n.trim().len() < MAX_ATTRIBUTION_LEN)
}

/// The body without quoted earlier replies and a trailing signature. Forwarded content is kept; only
/// the text after an "On ... wrote:" line, `>` lines and a `-- ` signature go. Conservative: if less
/// than [`MIN_STRIPPED_LEN`] characters would remain (not counting the leading `From:` line) the
/// original comes back.
pub fn strip_quoted(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if is_attribution(line, lines.get(i + 1).copied()) {
            break;
        }
        if line.trim_start().starts_with('>') {
            continue;
        }
        kept.push(line);
    }
    if let Some(sig) = kept.iter().rposition(|l| l.trim_end() == "--")
        && kept.len() - sig - 1 <= MAX_SIGNATURE_LINES
    {
        kept.truncate(sig);
    }
    let out = kept.join("\n");
    let content = |s: &str| -> usize {
        let s = s.trim();
        let rest = if s.starts_with("From:") { s.split_once('\n').map_or("", |(_, r)| r) } else { s };
        rest.trim().chars().count()
    };
    if content(&out) < MIN_STRIPPED_LEN { body.to_string() } else { out.trim_end().to_string() }
}

/// A subject for comparing reminders: lowercase, leading `Re:`/`Fwd:`/`Fw:` removed, whitespace collapsed.
pub fn normalize_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let Some(prefix) = ["re:", "fwd:", "fw:"].into_iter().find(|p| lower.starts_with(p)) else { break };
        s = s[prefix.len()..].trim_start();
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The bare lowercase address of a `From` header: `Ada <Ada@X.org>` -> `ada@x.org`.
pub fn sender_address(from: &str) -> String {
    let inner = match (from.rfind('<'), from.rfind('>')) {
        (Some(a), Some(b)) if a < b => &from[a + 1..b],
        _ => from,
    };
    inner.trim().trim_matches('"').to_lowercase()
}

/// The `From` header of an email record: its payload, or the first line of its body text.
pub fn sender_of(payload: &Value, body_text: &str) -> String {
    payload
        .get("from")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| body_text.lines().next().and_then(|l| l.strip_prefix("From:")).map(|s| s.trim().to_string()))
        .unwrap_or_default()
}

/// Gmail category labels and `SENT` from a record's payload (`CATEGORY_UPDATES`, ...).
pub fn labels_of(payload: &Value) -> Vec<String> {
    payload
        .get("label_ids")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter(|l| l.starts_with("CATEGORY_") || *l == "SENT").map(str::to_string).collect())
        .unwrap_or_default()
}

const CALENDAR_SUBJECT_PREFIXES: &[&str] =
    &["invitation:", "updated invitation:", "accepted:", "declined:", "tentatively accepted:", "invitation from google calendar"];

/// The connector kind whose own records already cover this email, if any: Ed and GitHub notification
/// mail, Google Calendar invitation mail. The caller skips extraction only when the owner has that
/// connector enabled.
pub fn duplicate_connector_kind(from: &str, subject: &str) -> Option<&'static str> {
    let addr = sender_address(from);
    let domain = addr.rsplit_once('@').map_or("", |(_, d)| d);
    if domain == "edstem.org" || domain.ends_with(".edstem.org") {
        return Some("ed");
    }
    if addr == "notifications@github.com" || addr == "noreply@github.com" {
        return Some("github");
    }
    let subj = subject.trim().to_lowercase();
    if addr == "calendar-notification@google.com" || CALENDAR_SUBJECT_PREFIXES.iter().any(|p| subj.starts_with(p)) {
        return Some("google_calendar");
    }
    None
}

/// Whether an earlier email (`candidates` are `(from header, subject)` pairs from the window) is the
/// same reminder: same sender address and same normalised subject.
pub fn is_repeat(from: &str, subject: &str, candidates: &[(String, String)]) -> bool {
    let (addr, subj) = (sender_address(from), normalize_subject(subject));
    candidates.iter().any(|(f, s)| sender_address(f) == addr && normalize_subject(s) == subj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_quoted_reply_after_attribution() {
        let body = "From: Ada <ada@x.org>\nYes, Thursday at 2pm works for me, see you in the lab.\n\nOn Mon, 3 Mar 2025 at 10:00, Sam <sam@y.org> wrote:\n> Can we meet Thursday?\n> Thanks";
        let out = strip_quoted(body);
        assert!(out.contains("Thursday at 2pm"));
        assert!(!out.contains("Can we meet"));
        assert!(!out.contains("wrote:"));
    }

    #[test]
    fn strips_wrapped_attribution_and_gt_lines() {
        let body = "From: a@x.org\nThe deadline is Friday 5pm, please send the form before then.\n> old text\nOn Mon, 3 Mar 2025 at 10:00, Sam Roberts <sam@y.org>\nwrote:\n> older";
        let out = strip_quoted(body);
        assert!(out.ends_with("before then."), "{out}");
        let gt = "From: a@x.org\nPlease read the attached policy before the session starts.\n> quoted\nThanks";
        assert_eq!(strip_quoted(gt), "From: a@x.org\nPlease read the attached policy before the session starts.\nThanks");
    }

    #[test]
    fn strips_trailing_signature_but_keeps_forwarded_content() {
        let body = "From: a@x.org\nI have approved your extension request until 14 March.\n-- \nAda Lovelace\nLecturer";
        assert_eq!(strip_quoted(body), "From: a@x.org\nI have approved your extension request until 14 March.");
        let fwd = "From: a@x.org\nFYI\n---------- Forwarded message ----------\nFrom: b@x.org\nSubject: Exam on 5 June in room 3";
        assert_eq!(strip_quoted(fwd), fwd);
    }

    #[test]
    fn keeps_original_when_little_would_remain() {
        let body = "From: a@x.org\nOk\nOn Mon, 3 Mar 2025, Sam wrote:\n> a long earlier message with the real content in it";
        assert_eq!(strip_quoted(body), body);
        assert_eq!(strip_quoted("short"), "short");
    }

    #[test]
    fn mid_message_dashes_are_not_a_signature() {
        let mut body = String::from("From: a@x.org\nSection one text that is long enough.\n--\n");
        body.push_str(&"more content line\n".repeat(20));
        assert_eq!(strip_quoted(&body), body.trim_end());
    }

    #[test]
    fn subject_normalisation() {
        assert_eq!(normalize_subject("  RE: Re: Fwd:   Reminder:  Quiz  due "), "reminder: quiz due");
        assert_eq!(normalize_subject("FW: x"), "x");
        assert_eq!(normalize_subject("Research update"), "research update");
        assert_eq!(normalize_subject("Reply needed"), "reply needed");
    }

    #[test]
    fn sender_addresses() {
        assert_eq!(sender_address("Ada L <Ada@X.org>"), "ada@x.org");
        assert_eq!(sender_address("ada@x.org"), "ada@x.org");
        assert_eq!(sender_address("\"Ed\" <no-reply@edstem.org>"), "no-reply@edstem.org");
        assert_eq!(sender_of(&json!({"from": "A <a@x.org>"}), ""), "A <a@x.org>");
        assert_eq!(sender_of(&json!({}), "From: B <b@x.org>\nhi"), "B <b@x.org>");
    }

    #[test]
    fn duplicate_connector_matching() {
        assert_eq!(duplicate_connector_kind("Ed <no-reply@edstem.org>", "New post"), Some("ed"));
        assert_eq!(duplicate_connector_kind("x@au.edstem.org", "s"), Some("ed"));
        assert_eq!(duplicate_connector_kind("Not Ed <a@notedstem.org>", "s"), None);
        assert_eq!(duplicate_connector_kind("GitHub <notifications@github.com>", "[repo] PR"), Some("github"));
        assert_eq!(duplicate_connector_kind("noreply@github.com", "Security alert"), Some("github"));
        assert_eq!(duplicate_connector_kind("a@b.org", "Updated invitation: Sync @ Mon"), Some("google_calendar"));
        assert_eq!(duplicate_connector_kind("a@b.org", "Tentatively accepted: Sync"), Some("google_calendar"));
        assert_eq!(duplicate_connector_kind("Google Calendar <calendar-notification@google.com>", "Reminder"), Some("google_calendar"));
        assert_eq!(duplicate_connector_kind("a@b.org", "Lunch invitation"), None);
    }

    #[test]
    fn repeat_matching_needs_same_sender_and_subject() {
        let earlier = vec![("Unit <unit@x.org>".to_string(), "Reminder: Quiz 2".to_string())];
        assert!(is_repeat("UNIT@x.org", "RE: reminder:  quiz 2", &earlier));
        assert!(!is_repeat("other@x.org", "Reminder: Quiz 2", &earlier));
        assert!(!is_repeat("unit@x.org", "Reminder: Quiz 3", &earlier));
        assert!(!is_repeat("unit@x.org", "Reminder: Quiz 2", &[]));
    }

    #[test]
    fn labels_keep_categories_and_sent() {
        let p = json!({"label_ids": ["INBOX", "CATEGORY_UPDATES", "IMPORTANT", "SENT"]});
        assert_eq!(labels_of(&p), vec!["CATEGORY_UPDATES".to_string(), "SENT".to_string()]);
        assert!(labels_of(&json!({})).is_empty());
    }
}
