//! What a model call costs: a price per million input and output tokens, per model, in US dollars.
//!
//! Provider prices change, so the table is data, not code. Three layers, first match wins:
//!
//! 1. **The operator's table** (`ai_price`, set with `PUT /api/operator/v1/ai/prices`): changes apply on every
//!    replica within [`CACHE_TTL`], with no deploy.
//! 2. **The defaults below**, for the models this server uses out of the box.
//! 3. **A deliberately high fallback** ([`FALLBACK`]) for any other model, logged once per model per process as a
//!    warning. A model nobody priced is never free: it eats the allowance quickly, which is the cue to price it.
//!
//! A dated snapshot (`gpt-4o-mini-2024-07-18`) uses the price of its family (`gpt-4o-mini`); nothing else matches
//! by prefix, so `gpt-4o-ultra` is not priced as `gpt-4o`.
//!
//! Cost is an integer number of micro-dollars (millionths of a dollar). A price in dollars per million tokens
//! times a token count is already in micro-dollars, so no unit conversion is needed, and the sum is rounded up.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long a process trusts the operator's price table before reading it again.
pub const CACHE_TTL: Duration = Duration::from_secs(30);

/// US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Price {
    /// Per million input (prompt) tokens.
    pub input_per_million_usd: f64,
    /// Per million output (completion) tokens; 0 for an embedding model.
    pub output_per_million_usd: f64,
}

impl Price {
    pub const fn new(input: f64, output: f64) -> Self {
        Price { input_per_million_usd: input, output_per_million_usd: output }
    }

    /// Whether the numbers can be a price: finite, not negative, and not absurd (a typo of a thousand times).
    pub fn valid(&self) -> bool {
        let ok = |v: f64| v.is_finite() && (0.0..=10_000.0).contains(&v);
        ok(self.input_per_million_usd) && ok(self.output_per_million_usd)
    }
}

/// Priced at several times the most expensive model in common use, so an unknown model can never look cheap.
pub const FALLBACK: Price = Price::new(30.0, 60.0);

/// The models this server uses out of the box, and the other common ones.
pub const DEFAULTS: &[(&str, Price)] = &[
    ("gpt-4o-mini", Price::new(0.15, 0.60)),
    ("gpt-4o", Price::new(2.50, 10.00)),
    ("gpt-4.1", Price::new(2.00, 8.00)),
    ("gpt-4.1-mini", Price::new(0.40, 1.60)),
    ("gpt-4.1-nano", Price::new(0.10, 0.40)),
    ("text-embedding-3-small", Price::new(0.02, 0.0)),
    ("text-embedding-3-large", Price::new(0.13, 0.0)),
    ("text-embedding-ada-002", Price::new(0.10, 0.0)),
];

/// A dated snapshot suffix: `-2024-07-18` or `-20240718`.
fn snapshot_family(model: &str) -> Option<&str> {
    let (family, tail) = model.rsplit_once('-')?;
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    if digits(tail, 8) {
        return Some(family);
    }
    // -YYYY-MM-DD
    let (rest, dd) = (family, tail);
    let (rest, mm) = rest.rsplit_once('-')?;
    let (family, yyyy) = rest.rsplit_once('-')?;
    (digits(yyyy, 4) && digits(mm, 2) && digits(dd, 2)).then_some(family)
}

/// The price table in force: the operator's rows over the defaults.
#[derive(Debug, Clone, Default)]
pub struct Table {
    overrides: BTreeMap<String, Price>,
}

/// A model's price and whether it was found (a model that was not is priced at [`FALLBACK`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Priced {
    pub price: Price,
    pub known: bool,
}

impl Table {
    pub fn with_overrides(overrides: BTreeMap<String, Price>) -> Self {
        Table { overrides }
    }

    pub fn overrides(&self) -> &BTreeMap<String, Price> {
        &self.overrides
    }

    fn exact(&self, model: &str) -> Option<Price> {
        self.overrides.get(model).copied().or_else(|| DEFAULTS.iter().find(|(m, _)| *m == model).map(|(_, p)| *p))
    }

    pub fn price(&self, model: &str) -> Priced {
        let model = model.trim();
        let found = self.exact(model).or_else(|| snapshot_family(model).and_then(|f| self.exact(f)));
        match found {
            Some(price) => Priced { price, known: true },
            None => Priced { price: FALLBACK, known: false },
        }
    }

    /// The whole table as it applies now: the defaults, with the operator's rows over them.
    pub fn effective(&self) -> BTreeMap<String, Price> {
        let mut all: BTreeMap<String, Price> = DEFAULTS.iter().map(|(m, p)| (m.to_string(), *p)).collect();
        all.extend(self.overrides.clone());
        all
    }
}

/// The cost of `input` and `output` tokens at `price`, in micro-dollars, rounded up so a small call is never free.
pub fn cost_micros(price: &Price, input: u64, output: u64) -> i64 {
    let micros = input as f64 * price.input_per_million_usd + output as f64 * price.output_per_million_usd;
    if micros <= 0.0 {
        return 0;
    }
    micros.ceil().min(i64::MAX as f64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_models_are_priced() {
        let t = Table::default();
        let p = t.price("gpt-4o-mini");
        assert!(p.known);
        assert_eq!(p.price, Price::new(0.15, 0.60));
        assert_eq!(t.price("text-embedding-3-small").price, Price::new(0.02, 0.0));
    }

    #[test]
    fn a_model_nobody_priced_is_never_free() {
        let t = Table::default();
        let p = t.price("gpt-6-luna");
        assert!(!p.known);
        assert_eq!(p.price, FALLBACK);
        // dearer than every default, both ways
        for (m, d) in DEFAULTS {
            assert!(FALLBACK.input_per_million_usd > d.input_per_million_usd, "{m}");
            assert!(FALLBACK.output_per_million_usd > d.output_per_million_usd, "{m}");
        }
        assert!(cost_micros(&p.price, 1, 0) > 0);
    }

    #[test]
    fn a_dated_snapshot_uses_its_family_and_nothing_else_matches_by_prefix() {
        let t = Table::default();
        assert_eq!(t.price("gpt-4o-mini-2024-07-18").price, Price::new(0.15, 0.60));
        assert_eq!(t.price("gpt-4o-2024-08-06").price, Price::new(2.50, 10.00));
        assert_eq!(t.price("text-embedding-3-small-20250101").price, Price::new(0.02, 0.0));
        assert!(!t.price("gpt-4o-ultra").known, "a lookalike is not gpt-4o");
        assert!(!t.price("gpt-4o-mini-preview").known);
        assert!(!t.price("gpt-4o-2024-13").known);
    }

    #[test]
    fn the_operators_table_wins_and_unlocks_a_model() {
        let mut o = BTreeMap::new();
        o.insert("gpt-6-luna".to_string(), Price::new(1.0, 2.0));
        o.insert("gpt-4o-mini".to_string(), Price::new(0.30, 1.20));
        let t = Table::with_overrides(o);
        assert_eq!(t.price("gpt-6-luna"), Priced { price: Price::new(1.0, 2.0), known: true });
        assert_eq!(t.price("gpt-4o-mini").price, Price::new(0.30, 1.20));
        assert_eq!(t.price("gpt-6-luna-2026-01-02").price, Price::new(1.0, 2.0));
        assert_eq!(t.effective().get("gpt-4o").copied(), Some(Price::new(2.50, 10.00)));
    }

    #[test]
    fn cost_is_micro_dollars_rounded_up() {
        // 1,000 input tokens of gpt-4o-mini: $0.00015 = 150 micro-dollars; 1,000 output: 600
        let p = Price::new(0.15, 0.60);
        assert_eq!(cost_micros(&p, 1_000, 0), 150);
        assert_eq!(cost_micros(&p, 1_000, 1_000), 750);
        assert_eq!(cost_micros(&p, 1, 0), 1, "a small call is never free");
        assert_eq!(cost_micros(&p, 0, 0), 0);
        assert_eq!(cost_micros(&Price::new(0.02, 0.0), 1_000_000, 0), 20_000);
    }

    #[test]
    fn a_price_must_be_a_sane_number() {
        assert!(Price::new(0.0, 0.0).valid());
        assert!(Price::new(2.5, 10.0).valid());
        assert!(!Price::new(-1.0, 1.0).valid());
        assert!(!Price::new(f64::NAN, 1.0).valid());
        assert!(!Price::new(1.0, f64::INFINITY).valid());
        assert!(!Price::new(50_000.0, 1.0).valid());
    }
}
