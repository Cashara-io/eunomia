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

/// Outlook-style quoting starts here: an "Original Message" rule, a line of underscores, or a
/// `From:` line followed within 4 lines by `Sent:` and then `Subject:` (or `To:`). `i > 0`: line 0 is
/// our own `From:` header.
fn is_outlook_quote(lines: &[&str], i: usize) -> bool {
    let t = lines[i].trim();
    if t.to_lowercase().starts_with("-----original message") {
        return true;
    }
    if t.len() >= 5 && t.chars().all(|c| c == '_') {
        return true;
    }
    if i == 0 || !t.starts_with("From:") {
        return false;
    }
    let ahead = &lines[i + 1..lines.len().min(i + 5)];
    let sent = ahead.iter().position(|l| l.trim_start().starts_with("Sent:"));
    sent.is_some_and(|k| ahead[k + 1..].iter().any(|l| l.trim_start().starts_with("Subject:") || l.trim_start().starts_with("To:")))
}

/// The body without quoted earlier replies and a trailing signature. Forwarded content is kept; only
/// the text after an "On ... wrote:" line, `>` lines and a `-- ` signature go. Conservative: if less
/// than [`MIN_STRIPPED_LEN`] characters would remain (not counting the leading `From:` line) the
/// original comes back.
pub fn strip_quoted(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if is_attribution(line, lines.get(i + 1).copied()) || is_outlook_quote(&lines, i) {
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

/// One email as the repeat check sees it. `at` is seconds since the epoch.
#[derive(Debug, Clone)]
pub struct Copy {
    pub from: String,
    pub subject: String,
    pub thread_id: Option<String>,
    pub at: i64,
}

/// Automated or list mail: a Promotions/Updates label or a no-reply style sender.
pub fn is_bulk(from: &str, labels: &[String]) -> bool {
    let addr = sender_address(from);
    let local = addr.split('@').next().unwrap_or("");
    labels.iter().any(|l| l == "CATEGORY_UPDATES" || l == "CATEGORY_PROMOTIONS")
        || ["no-reply", "noreply", "donotreply", "do-not-reply", "mailer-daemon"].iter().any(|p| local.contains(p))
}

/// Whether `this` is a repeat reminder to skip: bulk mail (never `SENT`) with an earlier copy (same
/// sender address and normalised subject) in a different thread, no more than [`REPEAT_WINDOW_DAYS`]
/// after the first such copy. Measured from the first copy, so a series of reminders does not chain.
pub fn is_repeat(this: &Copy, labels: &[String], candidates: &[Copy]) -> bool {
    if labels.iter().any(|l| l == "SENT") || !is_bulk(&this.from, labels) {
        return false;
    }
    let (addr, subj) = (sender_address(&this.from), normalize_subject(&this.subject));
    let first = candidates
        .iter()
        .filter(|c| c.at < this.at && sender_address(&c.from) == addr && normalize_subject(&c.subject) == subj)
        .filter(|c| c.thread_id.is_none() || this.thread_id.is_none() || c.thread_id != this.thread_id)
        .map(|c| c.at)
        .min();
    first.is_some_and(|f| this.at - f <= REPEAT_WINDOW_DAYS * 86_400)
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

    const DAY: i64 = 86_400;

    fn copy(from: &str, subject: &str, thread: &str, day: i64) -> Copy {
        Copy { from: from.into(), subject: subject.into(), thread_id: Some(thread.into()), at: day * DAY }
    }

    fn updates() -> Vec<String> {
        vec!["CATEGORY_UPDATES".to_string()]
    }

    #[test]
    fn a_bulk_reminder_in_another_thread_inside_the_window_is_skipped() {
        let earlier = [copy("Unit <unit@x.org>", "Reminder: Quiz 2", "t1", 0)];
        let now = copy("UNIT@x.org", "RE: reminder:  quiz 2", "t2", 5);
        assert!(is_repeat(&now, &updates(), &earlier));
        assert!(is_repeat(&copy("Bot <noreply@x.org>", "Reminder: Quiz 2", "t2", 5), &[], &[copy("Bot <noreply@x.org>", "Reminder: Quiz 2", "t1", 0)]));
    }

    #[test]
    fn a_same_thread_reply_is_not_skipped() {
        let earlier = [copy("Unit <unit@x.org>", "Reminder: Quiz 2", "t1", 0)];
        assert!(!is_repeat(&copy("Unit <unit@x.org>", "Re: Reminder: Quiz 2", "t1", 1), &updates(), &earlier));
    }

    #[test]
    fn sent_and_personal_mail_is_not_skipped() {
        let earlier = [copy("Unit <unit@x.org>", "Reminder: Quiz 2", "t1", 0)];
        let now = copy("Unit <unit@x.org>", "Reminder: Quiz 2", "t2", 1);
        let sent = vec!["CATEGORY_UPDATES".to_string(), "SENT".to_string()];
        assert!(!is_repeat(&now, &sent, &earlier));
        assert!(!is_repeat(&now, &[], &earlier), "a person writing twice is not bulk");
        assert!(!is_repeat(&now, &updates(), &[]));
        assert!(!is_repeat(&copy("other@x.org", "Reminder: Quiz 2", "t2", 1), &updates(), &earlier));
        assert!(!is_repeat(&copy("Unit <unit@x.org>", "Reminder: Quiz 3", "t2", 1), &updates(), &earlier));
    }

    #[test]
    fn reminders_do_not_chain_past_the_window_from_the_first_copy() {
        let earlier = [copy("Unit <unit@x.org>", "Reminder", "t1", 0), copy("Unit <unit@x.org>", "Reminder", "t2", 10)];
        assert!(is_repeat(&copy("Unit <unit@x.org>", "Reminder", "t3", 12), &updates(), &earlier));
        assert!(!is_repeat(&copy("Unit <unit@x.org>", "Reminder", "t4", 20), &updates(), &earlier), "20 days after the first copy");
    }

    #[test]
    fn outlook_quoting_is_cut() {
        let tail = "From: a@x.org\nPlease confirm the booking by Friday, thanks very much.\n";
        let orig = format!("{tail}\n-----Original Message-----\nFrom: Sam\nSent: Monday\nSubject: Booking\nold text");
        assert_eq!(strip_quoted(&orig), tail.trim_end());
        let rule = format!("{tail}\n________________________________\nFrom: Sam <s@y.org>\nold");
        assert_eq!(strip_quoted(&rule), tail.trim_end());
        let block = format!("{tail}\nFrom: Sam Roberts <s@y.org>\nSent: Monday, 3 March 2025 10:00\nTo: Ada\nSubject: Booking\nold text");
        assert_eq!(strip_quoted(&block), tail.trim_end());
        let sent_to = format!("{tail}\nFrom: Sam\nSent: Monday\nCc: x\nTo: Ada\nold");
        assert_eq!(strip_quoted(&sent_to), tail.trim_end());
        // a forwarded header without "Sent:" is kept
        let fwd = format!("{tail}From: b@x.org\nSubject: Exam on 5 June");
        assert_eq!(strip_quoted(&fwd), fwd.trim_end());
    }

    #[test]
    fn labels_keep_categories_and_sent() {
        let p = json!({"label_ids": ["INBOX", "CATEGORY_UPDATES", "IMPORTANT", "SENT"]});
        assert_eq!(labels_of(&p), vec!["CATEGORY_UPDATES".to_string(), "SENT".to_string()]);
        assert!(labels_of(&json!({})).is_empty());
    }
}
