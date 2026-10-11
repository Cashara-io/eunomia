//! Test-only switches for the isolation proof (`tests/isolation.rs`, docs/architecture/tenancy.md).
//! Compiled only with the `test-support` feature, so a release binary cannot turn a filter off.
//!
//! * [`set_no_app_filters`] (or `ISOLATION_TEST_NO_APP_FILTERS=1`) rewrites every org-database
//!   statement so its owner and vault filters match everything. With them gone, only the database
//!   wall (one database per org, one database user per org) is left standing.
//! * [`mutate`] edits one named statement's text, to prove the suite goes red when a filter really
//!   is missing.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static NO_APP_FILTERS: AtomicBool = AtomicBool::new(false);
static REWRITTEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Statements rewritten so far (by the filter switch or a mutation): a run that rewrote none proved nothing.
pub fn rewritten() -> u64 {
    REWRITTEN.load(Ordering::Relaxed)
}
static MUTATIONS: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

/// The filter predicates the app puts in org-database statements. Only a predicate is rewritten
/// (after WHERE, AND, OR or an opening parenthesis), never an assignment (`SET vault = $vault`).
const FILTERS: &[&str] = &["owner = $owner", "vault = $vault", "user = $user", "vault IN $vaults"];
const LEADERS: &[&str] = &["WHERE ", "AND ", "OR ", "("];

pub fn set_no_app_filters(on: bool) {
    NO_APP_FILTERS.store(on, Ordering::SeqCst);
}

fn no_app_filters() -> bool {
    NO_APP_FILTERS.load(Ordering::Relaxed) || std::env::var("ISOLATION_TEST_NO_APP_FILTERS").is_ok_and(|v| v == "1")
}

/// In statement `stmt`, replace `from` with `to`. Cleared by [`clear_mutations`].
pub fn mutate(stmt: &str, from: &str, to: &str) {
    MUTATIONS.lock().unwrap().push((stmt.into(), from.into(), to.into()));
}

pub fn clear_mutations() {
    MUTATIONS.lock().unwrap().clear();
}

// ponytail: textual rewrite of the four predicate shapes the app uses; a new filter shape needs adding to FILTERS
// (the layer-removal runs would then show it still filtering).
pub(crate) fn transform<'a>(name: &str, sql: &'a str, org_scope: bool) -> Cow<'a, str> {
    let mut out = Cow::Borrowed(sql);
    if org_scope && no_app_filters() {
        for (f, lead) in FILTERS.iter().flat_map(|f| LEADERS.iter().map(move |l| (f, l))) {
            let pat = format!("{lead}{f}");
            if out.contains(&pat) {
                out = Cow::Owned(out.replace(&pat, &format!("{lead}true")));
            }
        }
    }
    if matches!(out, Cow::Owned(_)) {
        REWRITTEN.fetch_add(1, Ordering::Relaxed);
    }
    let muts = MUTATIONS.lock().unwrap();
    for (stmt, from, to) in muts.iter() {
        if stmt == name && out.contains(from.as_str()) {
            out = Cow::Owned(out.replace(from.as_str(), to));
            REWRITTEN.fetch_add(1, Ordering::Relaxed);
        }
    }
    out
}
