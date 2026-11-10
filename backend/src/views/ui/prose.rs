//! Authorise complete numeric spans in final prose, at the server boundary.
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use chrono::{DateTime, NaiveDate};
use regex::Regex;
use serde_json::Value;
use surrealdb::types::SurrealValue;
use unicode_normalization::UnicodeNormalization;
use crate::error::AppResult;
use super::{bad, trusted_number};

pub(super) fn field(key: &str) -> bool {
    matches!(key, "title" | "summary" | "text" | "label" | "headline" | "note" | "reason" | "caption" | "legend" | "why")
}
fn display(text: &str) -> String {
    static FORMAT: OnceLock<Regex> = OnceLock::new();
    static DIGIT: OnceLock<Regex> = OnceLock::new();
    let format = FORMAT.get_or_init(|| Regex::new(r"[\p{Cf}\p{Mn}\p{Me}]").unwrap());
    let digit = DIGIT.get_or_init(|| Regex::new(r"^\p{Nd}$").unwrap());
    let compatible: String = text.nfkc().collect();
    let stripped = format.replace_all(&compatible, "");
    stripped.chars().map(|c| {
        if c.is_whitespace() { return ' '; }
        if matches!(c, '\u{2212}' | '\u{2012}'..='\u{2015}' | '\u{fe63}' | '\u{ff0d}') { return '-'; }
        if c.is_ascii() || !digit.is_match(&c.to_string()) { return c; }
        let mut start = c as u32;
        while start > 0 && char::from_u32(start - 1).is_some_and(|p| digit.is_match(&p.to_string())) { start -= 1; }
        char::from_digit((c as u32 - start) % 10, 10).unwrap()
    }).collect::<String>().chars().fold(String::new(), |mut out, c| {
        // Match collapsed browser whitespace without trimming substitution boundaries.
        if c != ' ' || !out.ends_with(' ') { out.push(c); }
        out
    })
}
fn normalise(text: &str) -> String { display(text).to_lowercase() }
const CURRENCY_CODE: &str = r"(?:aed|afn|all|amd|ang|aoa|ars|aud|awg|azn|bam|bbd|bdt|bgn|bhd|bif|bmd|bnd|bob|brl|bsd|btn|bwp|byn|bzd|cad|cdf|chf|clp|cny|cop|crc|cuc|cup|cve|czk|djf|dkk|dop|dzd|egp|ern|etb|eur|fjd|fkp|gbp|gel|ghs|gip|gmd|gnf|gtq|gyd|hkd|hnl|hrk|htg|huf|idr|ils|inr|iqd|irr|isk|jmd|jod|jpy|kes|kgs|khr|kmf|kpw|krw|kwd|kyd|kzt|lak|lbp|lkr|lrd|lsl|lyd|mad|mdl|mga|mkd|mmk|mnt|mop|mru|mur|mvr|mwk|mxn|myr|mzn|nad|ngn|nio|nok|npr|nzd|omr|pab|pen|pgk|php|pkr|pln|pyg|qar|ron|rsd|rub|rwf|sar|sbd|scr|sdg|sek|sgd|shp|sle|sll|sos|srd|ssp|stn|svc|syp|szl|thb|tjs|tmt|tnd|top|try|ttd|twd|tzs|uah|ugx|usd|uyu|uzs|ves|vnd|vuv|wst|xaf|xag|xau|xba|xbb|xbc|xbd|xcd|xcg|xdr|xof|xpd|xpf|xpt|xst|xsu|xts|xua|xxx|yer|zar|zmw|zwg|zwl)";
// Byte ranges in the normalised display. Whitespace cannot detach numeric modifiers.
// Ordinary words, including "per cent", terminate a span.
fn spans(text: &str) -> Vec<std::ops::Range<usize>> {
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    let number = NUMBER.get_or_init(|| {
        // ISO currency codes from the local ICU currency enumeration, plus fund and metal codes.
        let code = CURRENCY_CODE;
        let iso = r"[0-9]{4}-[0-9]{2}-[0-9]{2}(?:t[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:z|[+-][0-9]{2}:[0-9]{2}))?";
        let modifier = r"[\p{Sc}+%‰-]";
        let connector = format!(r"(?:{modifier} *|[/\x{{2044}}e])");
        let tail = format!(r"(?: *{modifier}|[/\x{{2044}}e]|\p{{N}}|[.,'_]\p{{N}})*");
        let iso_range = format!(r"{iso}(?:(?:/|\.\.|-){iso})?");
        let part = format!(r"(?:{iso_range}|{connector}*[.,'_]?\p{{N}}{tail})");
        Regex::new(&format!(r"(?:\b{code} *)?(?:{part})+(?:(?: \p{{N}}{tail})|(?: *{code}\b{tail}))*")).unwrap()
    });
    number.find_iter(text).filter(|m| m.as_str().chars().any(char::is_numeric)).map(|m| m.range()).collect()
}

pub(super) fn identifier(raw: &str) -> AppResult<()> {
    let valid = !raw.is_empty()
        && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && raw.chars().any(|c| c.is_ascii_alphabetic())
        && !raw.trim_start_matches('_').starts_with(|c: char| c.is_ascii_digit())
        && spans(&normalise(raw)).iter().all(|span| {
            // The prose scanner may consume an earlier e as an exponent connector.
            let start = span.start + raw[span.clone()].bytes().take_while(|c| !c.is_ascii_digit()).count();
            raw[..start].chars().next_back().is_some_and(|c| c.is_ascii_alphabetic())
                || raw[span.end..].chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        });
    if !valid { return Err(bad("identifier must use ASCII letters and underscores, with numeric spans attached to letters")); }
    Ok(())
}

fn token(c: char) -> bool { currency(c) || c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.' | '%' | '+' | '\u{2044}') }
fn following_token(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some('.') => chars.next().is_some_and(|c| c.is_ascii_digit()),
        Some(c) => token(c),
        None => false,
    }
}
fn date(s: &str) -> Option<String> {
    // chrono accepts unpadded fields. Require the canonical ISO spelling too.
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().filter(|d| d.to_string() == s).map(|d| d.to_string())
        .or_else(|| DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&chrono::Utc).to_rfc3339()))
}
#[derive(Default)]
pub(super) struct Allowed { strings: HashSet<String>, dates: HashSet<String> }
impl Allowed {
    fn string(&mut self, text: &str) {
        if text.chars().any(char::is_numeric) { self.strings.insert(text.into()); }
    }

    fn entities(&mut self, value: &Value) {
        match value {
            Value::Object(m) => for (k, v) in m {
                if matches!(k.as_str(), "name" | "kind" | "label" | "property" | "metric") && let Some(s) = v.as_str() { self.string(s); }
                else if v.is_object() || v.is_array() { self.entities(v); }
            },
            Value::Array(a) => for v in a { self.entities(v); },
            _ => (),
        }
    }
    fn from_data(data: &Value) -> Self {
        let mut out = Self::default();
        for source in data["sources"].as_object().into_iter().flat_map(|m| m.values()) {
            for k in ["kind", "entities", "relations", "start", "affected", "result"] {
                if let Some(s) = source[k].as_str() { out.string(s); } else { out.entities(&source[k]); }
            }
            for k in ["kind", "metric"] { if let Some(s) = source["metric"][k].as_str() { out.string(s); } }
            {
                for s in [&source["parameters"]["as_of"], &source["metric"]["as_of"], &source["result"]["as_of"]] {
                    if let Some(d) = s.as_str().and_then(|s| date(&normalise(s))) { if let Some((day, _)) = d.split_once('T') { out.dates.insert(day.into()); }
                        out.dates.insert(d); }
                }
            }
            let columns = source["columns"].as_array();
            for c in columns.into_iter().flatten() {
                for k in ["key", "label"] { if let Some(s) = c[k].as_str() { out.string(s); } }
            }
            for row in source["rows"].as_array().into_iter().flatten() {
                if let Some(s) = row["name"].as_str() { out.string(s); }
                for c in columns.into_iter().flatten() {
                    let key = c["key"].as_str().unwrap_or("");
                    if matches!(c["datatype"].as_str(), Some("text" | "enum" | "string" | "link")) {
                        if let Some(s) = row[key].as_str() { out.string(s); }
                        else { out.entities(&row[key]); }
                    } else if source["bucket_column"] == key && matches!(c["datatype"].as_str(), Some("date" | "datetime")) && let Some(s) = row[key].as_str() {
                        if let Some(d) = date(&normalise(s)) { if let Some((day, _)) = d.split_once('T') { out.dates.insert(day.into()); }
                        out.dates.insert(d); }
                        else { out.string(s); }
                    }
                }
            }
            // Dataset resolution records these before aggregation removes filter columns.
            for value in source["applied_dates"].as_array().into_iter().flatten() {
                if let Some(d) = value.as_str().and_then(|s| date(&normalise(s))) {
                    if let Some((day, _)) = d.split_once('T') { out.dates.insert(day.into()); }
                    out.dates.insert(d);
                }
            }
        }
        out
    }
    fn check(&self, raw: &str, substitutions: &[(std::ops::Range<usize>, String)], path: &str) -> AppResult<()> {
        let text = normalise(raw);
        let numeric = spans(&text);
        let bindings: Vec<_> = substitutions.iter().map(|(range, value)| {
            (normalise(&raw[..range.start]).len()..normalise(&raw[..range.end]).len(), normalise(value))
        }).collect();
        let mut server_ranges: HashMap<std::ops::Range<usize>, HashSet<usize>> = HashMap::new();
        for (source, server) in self.strings.iter().enumerate() {
            let server = normalise(server);
            let server_spans = spans(&server);
            for phrase in phrases(&server) {
                for (source_start, _) in server.match_indices(&phrase) {
                    for (start, _) in text.match_indices(&phrase) {
                        let end = start + phrase.len();
                        if text[..start].chars().next_back().is_some_and(token) || following_token(&text[end..]) { continue; }
                        for span in &server_spans {
                            if span.start >= source_start && span.end <= source_start + phrase.len() {
                                let range = start + span.start - source_start..start + span.end - source_start;
                                server_ranges.entry(range).or_default().insert(source);
                            }
                        }
                    }
                }
            }
        }
        for span in &numeric {
            let overlaps: Vec<_> = bindings.iter().filter(|(range, _)| range.start < span.end && span.start < range.end).collect();
            let authorised = if !overlaps.is_empty() {
                // Preserve main's exact-span rule, including its bare-number behaviour.
                // Only a complete custom-unit binding at a clause boundary is exempt.
                overlaps.len() == 1 && (overlaps[0].0 == *span && overlaps[0].1 == text[span.clone()]
                    || (span.start >= overlaps[0].0.start
                            // The scanner also consumes an opening single quote as a
                            // possible numeric separator. Keep a standalone quote as punctuation.
                            || text.get(span.start..overlaps[0].0.start) == Some("'")
                                && !text[..span.start].chars().next_back().is_some_and(token))
                        && span.end <= overlaps[0].0.end
                        && complete_custom_binding(raw, substitutions, &text, &overlaps[0].0, &overlaps[0].1))
            } else if iso_span(&text[span.clone()]).is_some() {
                !text[..span.start].chars().next_back().is_some_and(token)
                    && !following_token(&text[span.end..])
                    && iso_span(&text[span.clone()]).is_some_and(|dates| dates.iter().all(|d| self.dates.contains(d)))
            } else {
                server_ranges.get(span).is_some_and(|sources| sources.len() == 1)
            };
            if !authorised {
                return Err(bad(format!("unsupported raw digits or combined numeric span in {path}: {raw:?}; use one bound placeholder for the complete formatted number. Raw digits require one verbatim letter-bearing phrase in a server string predating the turn, or a matching applied ISO date or bucket label. Number words are unchecked.")));
            }
        }
        Ok(())
    }

}
// The scanner stops before custom units. Accept the whole rendered value only
// at a structural boundary, never before another word or a dash (a dash can
// join a suffix such as -squared). Inspect raw whitespace
// because normalise collapses line breaks into spaces.
fn complete_custom_binding(raw: &str, substitutions: &[(std::ops::Range<usize>, String)], text: &str, range: &std::ops::Range<usize>, value: &str) -> bool {
    if text.get(range.clone()) != Some(value)
        || display(&text[..range.start]).chars().next_back().is_some_and(token)
        || spans(value).first().is_some_and(|span| *span == (0..value.len()))
    { return false; }
    let Some((original, _)) = substitutions.iter().find(|(r, _)| normalise(&raw[..r.start]).len() == range.start) else { return false; };
    match raw[original.end..].trim_start_matches(|c: char| c.is_whitespace() && !matches!(c, '\n' | '\r')).chars().next() {
        None => true,
        Some(c) => matches!(c, '.' | ',' | ';' | ':' | ')' | ']' | '}' | '\'' | '"' | '?' | '!' | '\n' | '\r'
            | '\u{2018}' | '\u{2019}' | '\u{201c}' | '\u{201d}'),
    }
}
// Dates are a single span, including ranges. No endpoint can license a prefix or suffix.
fn iso_span(text: &str) -> Option<Vec<String>> {
    static ISO: OnceLock<Regex> = OnceLock::new();
    let iso = ISO.get_or_init(|| Regex::new(r"[0-9]{4}-[0-9]{2}-[0-9]{2}(?:t[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:z|[+-][0-9]{2}:[0-9]{2}))?").unwrap());
    let mut rest = text;
    let mut dates = Vec::new();
    loop {
        let m = iso.find(rest).filter(|m| m.start() == 0)?;
        dates.push(date(m.as_str())?);
        rest = &rest[m.end()..];
        if rest.is_empty() { return Some(dates); }
        if dates.len() == 2 { return None; }
        rest = rest.strip_prefix("..").or_else(|| rest.strip_prefix('/')).or_else(|| rest.strip_prefix('-'))?;
    }
}
// A digit must travel with an adjacent letter-bearing word from one server string.
// Never join words from different records, or authorise a bare numeric token.
fn phrases(server: &str) -> Vec<String> {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let word = WORD.get_or_init(|| Regex::new(r"[\p{L}\p{N}_/%.+\x{2044}-]+").unwrap());
    let server = server.to_string();
    let words: Vec<_> = word.find_iter(&server).collect();
    let mut out = Vec::new();
    for span in spans(&server) {
        let mut start = span.start;
        let mut end = span.end;
        for (i, c) in server[..start].char_indices().rev() {
            if !token(c) { break; }
            start = i;
        }
        for (i, c) in server[end..].char_indices() {
            if !token(c) { break; }
            end = span.end + i + c.len_utf8();
        }
        if server[start..end].chars().any(char::is_alphabetic) { out.push(server[start..end].into()); }
        for w in &words {
            if !w.as_str().chars().any(char::is_alphabetic) { continue; }
            if w.end() <= start && server[w.end()..start].chars().all(char::is_whitespace) {
                out.push(server[w.start()..end].into());
            }
            if w.start() >= end && server[end..w.start()].chars().all(char::is_whitespace) {
                out.push(server[start..w.end()].into());
            }
        }
    }
    out
}

pub(super) async fn eligible(db: &crate::pool::OrgDb, owner: &surrealdb::types::RecordId, data: &Value, before: surrealdb::types::Datetime) -> AppResult<Allowed> {
    let mut allowed = Allowed::from_data(data);
    if allowed.strings.is_empty() { return Ok(allowed); }
    let vaults = crate::vaults::service::accessible_vault_ids(db, owner).await?;
    let mut result = crate::store::ontology::UI_PROSE_STRINGS.on(db)
        .bind(("vaults", vaults)).bind(("before", before))
        .bind(("strings", allowed.strings.iter().cloned().collect::<Vec<_>>())).await?;
    #[derive(serde::Deserialize, surrealdb::types::SurrealValue)]
    struct Row {
        #[surreal(default)] name: Option<String>,
        #[surreal(default)] kind: Option<String>,
        #[surreal(default)] label: Option<String>,
        #[surreal(default)] value_text: Option<String>,
    }
    let mut eligible = HashSet::new();
    for slot in 0..result.num_statements() {
        let rows: Vec<Row> = result.take(slot)?;
        for row in rows { eligible.extend([row.name, row.kind, row.label, row.value_text].into_iter().flatten()); }
    }
    allowed.strings.retain(|s| eligible.contains(s));
    Ok(allowed)
}

fn pointer(binding: &str) -> AppResult<String> {
    if binding.starts_with("/sources/") {
        if binding.split('/').any(|s| s.contains('~') && {
            let mut chars = s.chars();
            let mut invalid = false;
            while let Some(c) = chars.next() { if c == '~' && !matches!(chars.next(), Some('0' | '1')) { invalid = true; } }
            invalid
        }) { return Err(bad("invalid JSON pointer escape in text placeholder")); }
        return Ok(binding.into());
    }
    static PATH: OnceLock<Regex> = OnceLock::new();
    let path = PATH.get_or_init(|| Regex::new(r"^sources\.[A-Za-z0-9_]+(?:\.[A-Za-z_][A-Za-z0-9_]*|\[(?:0|[1-9][0-9]*)\])+$").unwrap());
    if !path.is_match(binding) { return Err(bad("invalid text placeholder; use {{/sources/name/rows/0/amount}} or {{sources.name.rows[0].amount}}")); }
    Ok(format!("/{}", binding.replace('[', ".").replace(']', "").replace('.', "/")))
}
fn binding_error(message: String, data: &Value) -> crate::error::AppError {
    fn visit(value: &Value, path: &str, data: &Value, out: &mut Vec<String>) {
        if out.len() >= 8 { return; }
        if value.as_f64().is_some_and(f64::is_finite) && trusted_number(data, path) {
            out.push(format!("{{{{{path}}}}}"));
        } else {
            match value {
                Value::Object(fields) => for (key, v) in fields { visit(v, &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")), data, out); },
                Value::Array(values) => for (i, v) in values.iter().enumerate() { visit(v, &format!("{path}/{i}"), data, out); },
                _ => (),
            }
        }
    }
    let mut valid = Vec::new();
    for (name, source) in data["sources"].as_object().into_iter().flat_map(|m| m.iter()) {
        for field in ["totals", "metric", "result", "total_rows", "rows"] {
            visit(&source[field], &format!("/sources/{name}/{field}"), data, &mut valid);
        }
    }
    bad(format!("{message}. Bind one exact non-null typed numeric or date path. Valid numeric placeholders for these resolved sources: {}", if valid.is_empty() { "none; select a numeric property or metric".into() } else { valid.join(", ") }))
}
fn bound(binding: &str, data: &Value) -> AppResult<String> {
    let path = pointer(binding)?;
    let value = data.pointer(&path).ok_or_else(|| binding_error(format!("text placeholder {{{{{binding}}}}} points at nothing"), data))?;
    let parts: Vec<_> = path.split('/').skip(1).map(|s| s.replace("~1", "/").replace("~0", "~")).collect();
    let source = &data["sources"][&parts[1]];
    let rest: Vec<_> = parts[2..].iter().map(String::as_str).collect();
    let column = match rest.as_slice() {
        ["rows", _, key] | ["totals", key, _] | ["distributions", key, ..] => source["columns"].as_array().into_iter().flatten().find(|c| c["key"] == *key),
        _ => None,
    };
    if trusted_number(data, &path) && let Some(n) = value.as_f64().filter(|n| n.is_finite()) {
        let count = matches!(rest.as_slice(), ["total_rows"] | ["totals", _, "count" | "distinct"] | ["metric", "n_entities"] | ["metric", "groups", _, "n_entities"] | ["metric", "series", _, "n"] | ["result", "n_changed"] | ["result", "baseline" | "scenario", _, "n_entities"] | ["distributions", _, "bins" | "density", _, "count"]);
        let unit = if count { None } else if let Some(c) = column { c["unit"].as_str() }
            else if matches!(rest.as_slice(), ["diagram", "nodes", _, "metrics", _, "value"]) {
                path.rsplit_once('/').and_then(|(parent, _)| data.pointer(parent)).and_then(|m| m["unit"].as_str())
            }
            else if rest.first() == Some(&"metric") { source["metric"]["unit"].as_str() }
            else if matches!(rest.last(), Some(&"delta_pct")) { Some("%") }
            else { rest.get(2).and_then(|i| i.parse::<usize>().ok()).and_then(|i| source["result"][rest[1]][i]["unit"].as_str()) };
        return Ok(crate::views::number(n, unit));
    }
    let typed_date = column.is_some_and(|c| matches!(c["datatype"].as_str(), Some("date" | "datetime"))) && matches!(rest.as_slice(), ["rows", _, _])
        || matches!(rest.as_slice(), ["metric", "as_of"] | ["parameters", "as_of"] | ["result", "as_of"]);
    if typed_date && let Some(s) = value.as_str() && date(s).is_some() { return Ok(s.into()); }
    Err(binding_error(format!("text placeholder {{{{{binding}}}}} must bind a non-null server-typed numeric or date value"), data))
}
type Substitutions = Vec<(std::ops::Range<usize>, String)>;
fn interpolate(raw: &str, data: &Value, field: &str) -> AppResult<(String, Substitutions)> {
    let mut out = String::new();
    let mut rest = raw;
    let mut substitutions = Vec::new();
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let end = rest[start+2..].find("}}").ok_or_else(|| bad(format!("unclosed text placeholder in {field}; use {{{{/sources/name/rows/0/amount}}}}")))? + start + 2;
        let value = bound(&rest[start+2..end], data)?;
        let offset = out.len();
        out.push_str(&value);
        substitutions.push((offset..out.len(), value));
        rest = &rest[end+2..];
    }
    if rest.contains("}}") { return Err(bad(format!("unexpected placeholder closing braces in {field}"))); }
    out.push_str(rest);
    Ok((out, substitutions))
}
fn render(raw: &str, data: &Value, allowed: &Allowed, field: &str) -> AppResult<String> {
    let (out, substitutions) = interpolate(raw, data, field)?;
    allowed.check(&out, &substitutions, field)?;
    Ok(display(&out))
}
fn currency(c: char) -> bool {
    static SYMBOL: OnceLock<Regex> = OnceLock::new();
    SYMBOL.get_or_init(|| Regex::new(r"^\p{Sc}$").unwrap()).is_match(&c.to_string())
}

pub(super) fn resolve(value: &Value, data: &Value) -> AppResult<Value> {
    resolve_allowed(value, data, &Allowed::from_data(data))
}
pub(super) fn resolve_allowed(value: &Value, data: &Value, allowed: &Allowed) -> AppResult<Value> {
    fn walk(value: &mut Value, path: &str, data: &Value, allowed: &Allowed) -> AppResult<()> {
        match value {
            Value::Object(map) => for (key, v) in map {
                let path = format!("{path}/{key}");
                if field(key) && let Some(s) = v.as_str() { *v = Value::String(render(s, data, allowed, &path)?); }
                else { walk(v, &path, data, allowed)?; }
            },
            Value::Array(a) => for (i, v) in a.iter_mut().enumerate() { walk(v, &format!("{path}/{i}"), data, allowed)?; },
            _ => (),
        }
        Ok(())
    }
    let mut rendered = value.clone();
    walk(&mut rendered, "", data, allowed)?;
    Ok(rendered)
}

/// A decimal value with its semantic unit. Decimal arithmetic preserves integer
/// identities beyond f64 precision and makes midpoint rounding deterministic.
#[derive(Clone, Debug, PartialEq)]
struct ChatNumber {
    value: rust_decimal::Decimal,
    precision: u32,
    scale: rust_decimal::Decimal,
    unit: ChatUnit,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum ChatUnit { Plain, Percent, Ratio, Money(String), Other(String) }
impl ChatUnit {
    fn parse(unit: &str) -> Self {
        static CODE: OnceLock<Regex> = OnceLock::new();
        let code = CODE.get_or_init(|| Regex::new(&format!("^{CURRENCY_CODE}$")).unwrap());
        let unit = normalise(unit).trim().to_string();
        match unit.as_str() {
            "" => Self::Plain,
            "%" | "percent" | "percentage" => Self::Percent,
            "ratio" => Self::Ratio,
            "$" | "€" | "£" | "¥" => Self::Money(unit),
            _ if code.is_match(&unit) => Self::Money(unit),
            _ => Self::Other(unit),
        }
    }
    fn compatible(&self, other: &Self) -> bool {
        if self == other { return true; }
        match (self, other) {
            (Self::Money(a), Self::Money(b)) => {
                let symbol = |code: &str| match code {
                    "usd" | "aud" | "cad" | "nzd" | "hkd" | "sgd" | "$" => "$",
                    "eur" | "€" => "€", "gbp" | "£" => "£", "jpy" | "cny" | "¥" => "¥", _ => "",
                };
                (a.chars().count() == 1 || b.chars().count() == 1)
                    && !symbol(a).is_empty() && symbol(a) == symbol(b)
            }
            (Self::Other(a), Self::Other(b)) => unit_form(a) == unit_form(b),
            _ => false,
        }
    }
}
fn unit_form(unit: &str) -> &str {
    match unit {
        "kilogram" | "kilograms" | "kgs" => "kg",
        "gram" | "grams" => "g",
        "litre" | "litres" | "liter" | "liters" => "l",
        "metre" | "metres" | "meter" | "meters" => "m",
        _ if unit.len() > 3 => unit.strip_suffix('s').unwrap_or(unit),
        _ => unit,
    }
}
fn chat_spans(text: &str) -> Vec<std::ops::Range<usize>> {
    static SUFFIX: OnceLock<Regex> = OnceLock::new();
    let suffix = SUFFIX.get_or_init(|| Regex::new(&format!(r"^ *(?:thousand|million|billion|k|m|b)\b(?: *(?:{CURRENCY_CODE}\b|[%‰]))?|^ *(?:per cent|percent|percentage)\b")).unwrap());
    let mut out = Vec::new();
    for mut span in spans(text) {
        // In ranking prose, "top 5" is a count. TOP is also an ISO currency code.
        if text[span.clone()].starts_with("top ") { span.start += 4; }
        if out.last().is_some_and(|r: &std::ops::Range<usize>| span.start < r.end) { continue; }
        if let Some(m) = suffix.find(&text[span.end..]) { span.end += m.end(); }
        // The whole-span scanner deliberately keeps ambiguous arithmetic together. Split only
        // a simple two-endpoint range here, leaving dates, fractions and exponents untouched.
        static RANGE: OnceLock<Regex> = OnceLock::new();
        let range = RANGE.get_or_init(|| Regex::new(&format!(r"^(?P<left>(?:\p{{Sc}}|{CURRENCY_CODE})? *[0-9]+(?:\.[0-9]+)?) *- *(?P<right>[0-9]+(?:\.[0-9]+)?(?: *{CURRENCY_CODE})?)$" )).unwrap());
        if let Some(c) = range.captures(&text[span.clone()]) {
            for part in ["left", "right"] {
                let m = c.name(part).unwrap();
                out.push(span.start + m.start()..span.start + m.end());
            }
        } else { out.push(span); }
    }
    out
}

fn coordinated(gap: &str) -> bool {
    matches!(gap.trim(), "and" | "or" | "," | ", and" | ", or" | "to" | "up to" | "-")
}

impl ChatNumber {
    fn parse(raw: &str, context: &ChatUnit) -> Option<Self> {
        static NUMBER: OnceLock<Regex> = OnceLock::new();
        let number = NUMBER.get_or_init(|| Regex::new(&format!(r"^(?P<sign>[+-]?) *(?P<prefix>\p{{Sc}}|{CURRENCY_CODE})? *(?P<inner>[+-]?)(?P<value>(?:[0-9][0-9,]*)(?:\.[0-9]+)?|\.[0-9]+) *(?P<scale>thousand|million|billion|k|m|b)? *(?P<suffix>%|‰|per cent|percent|percentage|\p{{Sc}}|{CURRENCY_CODE})?$" )).unwrap());
        let c = number.captures(raw.trim())?;
        let digits = c.name("value")?.as_str().replace(',', "");
        let sign = c.name("sign").map(|s| s.as_str()).unwrap_or("");
        let inner = c.name("inner").map(|s| s.as_str()).unwrap_or("");
        if !sign.is_empty() && !inner.is_empty() { return None; }
        // from_str rounds excess fractional digits. Never let parsing change a claim.
        let exact_digits = if digits.contains('.') { digits.trim_end_matches('0').trim_end_matches('.') } else { &digits };
        let value = rust_decimal::Decimal::from_str_exact(&format!("{sign}{inner}{exact_digits}")).ok()?;
        let precision = digits.split_once('.').map(|(_,s)| s.len() as u32).unwrap_or(0);
        let scale = match c.name("scale").map(|s| s.as_str()) {
            Some("k" | "thousand") => 1000, Some("m" | "million") => 1_000_000,
            Some("b" | "billion") => 1_000_000_000, _ => 1,
        }.into();
        let prefix = c.name("prefix").map(|s| ChatUnit::parse(s.as_str()));
        let suffix = c.name("suffix").map(|s| match s.as_str() { "per cent" => ChatUnit::Percent, "‰" => ChatUnit::Other("‰".into()), s => ChatUnit::parse(s) });
        if let (Some(a),Some(b)) = (&prefix,&suffix) && !a.compatible(b) { return None; }
        let unit = match (prefix, suffix) {
            (Some(ChatUnit::Money(symbol)), Some(code)) if symbol.chars().count() == 1 => code,
            (Some(unit), _) | (_, Some(unit)) => unit,
            _ => context.clone(),
        };
        Some(Self { value, precision, scale, unit })
    }
    fn supports(&self, shown: &Self) -> bool {
        use rust_decimal::{Decimal, RoundingStrategy};
        let Some(mut value) = self.value.checked_mul(self.scale) else { return false; };
        if self.unit == ChatUnit::Ratio && shown.unit == ChatUnit::Percent {
            let Some(percent) = value.checked_mul(Decimal::from(100)) else { return false; }; value = percent;
        } else if !self.unit.compatible(&shown.unit) { return false; }
        let Some(value) = value.checked_div(shown.scale) else { return false; };
        // Rounding may not erase every non-zero significant digit.
        (self.value.is_zero() || !shown.value.is_zero())
            && value.round_dp_with_strategy(shown.precision.min(28), RoundingStrategy::MidpointAwayFromZero) == shown.value
    }
}

/// Current-turn evidence only. Chat renders this checked tree directly.
#[derive(Default)]
pub(crate) struct ChatProse {
    numbers: Vec<ChatNumber>,
    exact: HashSet<String>,
    sources: serde_json::Map<String, Value>,
}
const UNVERIFIED: &str = "[unverified number]";
const NUMBER_NOTE: &str = "Some numbers couldn't be checked against tool results.";
// Text nodes are grouped by the actual tree's paragraphs, headings, cells and items.
// A navigation autolink's URL is exempt prose. It breaks the checking unit so
// adjacent digits cannot borrow its URL. Hrefs are never visited or rewritten.
fn tree_units<'a>(nodes: &'a mut [crate::chat::tree::ChatNode], current: &mut Vec<&'a mut String>, units: &mut Vec<Vec<&'a mut String>>) {
    use crate::chat::tree::ChatNode;
    for node in nodes {
        if matches!(node, ChatNode::Link { href, autolink: true, .. } if crate::chat::tree::navigation_autolink(href)) {
            units.push(std::mem::take(current));
        } else if node.is_unit() {
            // Entering and leaving nested blocks must not join their neighbours.
            units.push(std::mem::take(current));
            let mut own = Vec::new();
            tree_units(node.children_mut(), &mut own, units);
            units.push(own);
        } else {
            match node {
                ChatNode::Text { text } => current.push(text),
                _ => tree_units(node.children_mut(), current, units),
            }
        }
    }
}

// Normalisation can change byte lengths. Retain each originating character.
fn unit_text(unit: &[&mut String]) -> (String, Vec<(usize, std::ops::Range<usize>)>) {
    let mut text = String::new();
    let mut origins = Vec::new();
    for (i, value) in unit.iter().enumerate() {
        for (offset,c) in value.char_indices() {
            for ch in normalise(&c.to_string()).chars() {
                if ch == ' ' && text.ends_with(' ') { continue; }
                text.push(ch);
                origins.extend(std::iter::repeat_n((i, offset..offset + c.len_utf8()), ch.len_utf8()));
            }
        }
    }
    (text, origins)
}

fn opaque_identifier(text: &str) -> bool {
    static OPAQUE: OnceLock<Regex> = OnceLock::new();
    OPAQUE.get_or_init(|| Regex::new(r"(?i)^(?:[a-z_][a-z_0-9]*:[^\s]+|[0-9a-f]{32,}|[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}|[0-9a-hjkmnp-tv-z]{26})$").unwrap()).is_match(text)
}

// Scan a measure once, including its unit. Unit digits never become separate claims.
fn chat_claims(text: &str, context: &ChatUnit, units: &[ChatUnit]) -> Vec<(std::ops::Range<usize>, Option<ChatNumber>)> {
    static DIGIT_UNIT: OnceLock<Regex> = OnceLock::new();
    let digit_unit = DIGIT_UNIT.get_or_init(|| Regex::new(r"^ *(?P<unit>per +[0-9]+ +[\p{L}]+|[\p{L}]+(?:\^?[0-9]+)?(?:/[\p{L}]+(?:\^?[0-9]+)?)+|[\p{L}]+\^?[0-9]+)\b").unwrap());
    // Recognise common physical units even when none of the evidence uses them. Otherwise
    // a currency prefix could incorrectly turn the unknown suffix in "$3 to 5 kg" into money.
    // A bare m can also mean million; evidence must identify it as a physical unit.
    let standard_units: Vec<_> = ["kg", "g", "mg", "t", "lb", "lbs", "oz", "l", "ml", "m", "km", "cm", "mm", "h", "hr", "hrs", "days", "hours"].into_iter().map(ChatUnit::parse).collect();
    let mut out = Vec::new();
    for mut span in chat_spans(text) {
        if out.last().is_some_and(|(r, _): &(std::ops::Range<usize>, Option<ChatNumber>)| span.start < r.end) { continue; }
        let base_end = spans(&text[span.clone()]).first().map_or(span.end, |r| span.start + r.end);
        let rest = &text[base_end..];
        let gap = rest.len() - rest.trim_start().len();
        let mut matched = digit_unit.captures(rest).map(|c| {
            (base_end + c.get(0).unwrap().end(), ChatUnit::parse(c.name("unit").unwrap().as_str()))
        });
        let decorated = ChatNumber::parse(&text[span.clone()], context).is_some_and(|n| matches!(n.unit, ChatUnit::Money(_) | ChatUnit::Percent));
        for (unit, known) in units.iter().map(|unit| (unit, true)).chain(standard_units.iter().map(|unit| (unit, false))) {
            let name = match unit { ChatUnit::Other(s) => s.as_str(), ChatUnit::Ratio => "ratio", _ => continue };
            let mut forms = vec![name.to_string(), unit_form(name).into(), format!("{}s", unit_form(name))];
            for alias in ["kg", "kilogram", "kilograms", "g", "gram", "grams", "l", "litre", "litres", "liter", "liters", "m", "metre", "metres", "meter", "meters"] {
                if unit_form(alias) == unit_form(name) { forms.push(alias.into()); }
            }
            for form in forms {
                if rest[gap..].starts_with(&form) {
                    let end = base_end + gap + form.len();
                    // "$5M" is a money scale, while the separate suffix in "$5 kg" conflicts.
                    if !known && form == "m" && end <= span.end { continue; }
                    if decorated && end <= span.end { continue; }
                    if text[end..].chars().next().is_some_and(|c| token(c) && c != '.') { continue; }
                    if matched.as_ref().is_none_or(|(old, _)| end > *old) { matched = Some((end, unit.clone())); }
                }
            }
        }
        let number = if let Some((end, unit)) = matched {
            let n = ChatNumber::parse(&text[span.start..base_end], &unit).filter(|n| n.unit.compatible(&unit));
            span.end = end;
            n
        } else { ChatNumber::parse(&text[span.clone()], context) };
        out.push((span, number));
    }
    // A trailing measure belongs to each bare member of the coordinated group. A typed
    // member stops propagation of a different unit; punctuation other than a list comma stops it.
    for i in (0..out.len().saturating_sub(1)).rev() {
        if !coordinated(&text[out[i].0.end..out[i + 1].0.start]) { continue; }
        let Some(unit) = out[i + 1].1.as_ref().map(|n| n.unit.clone()).filter(|u| *u != ChatUnit::Plain) else { continue; };
        if let Some(number) = &mut out[i].1 && number.unit == ChatUnit::Plain { number.unit = unit; }
    }
    // A currency prefix also applies forwards, including through an already inherited member.
    for i in 1..out.len() {
        if !coordinated(&text[out[i - 1].0.end..out[i].0.start]) { continue; }
        let Some(ChatUnit::Money(code)) = out[i - 1].1.as_ref().map(|n| &n.unit) else { continue; };
        let unit = ChatUnit::Money(code.clone());
        if let Some(number) = &mut out[i].1 && number.unit == ChatUnit::Plain { number.unit = unit; }
    }
    out
}

impl ChatProse {
    fn evidence(&mut self, text: &str, unit: &ChatUnit) {
        let text = normalise(text);
        static WORD: OnceLock<Regex> = OnceLock::new();
        let opaque: Vec<_> = WORD.get_or_init(|| Regex::new(r"\S+").unwrap()).find_iter(&text)
            .filter(|m| opaque_identifier(m.as_str().trim_matches(['(', ')', '[', ']', '{', '}', ',', '.', ';', '\'', '"'])))
            .map(|m| m.range()).collect();
        for (span, number) in chat_claims(&text, unit, std::slice::from_ref(unit)) {
            if opaque.iter().any(|id| id.start < span.end && span.start < id.end) { continue; }
            if let Some(n) = number { self.numbers.push(n); }
            else { self.exact.insert(text[span].into()); }
        }
    }
    pub(crate) fn text(&mut self, text: &str) { self.evidence(text, &ChatUnit::Plain); }
    pub(crate) fn result(&mut self, value: &Value) {
        self.collect(value, &ChatUnit::Plain, &HashMap::new());
        if let Some(sources) = value.pointer("/data/sources").and_then(Value::as_object) { self.sources.extend(sources.clone()); }
    }
    /// Only server-computed fields are collected. Request echoes stay in the API result but
    /// their declared paths are absent from this evidence projection. Argument values play no role.
    pub(crate) fn tool_result(&mut self, name: &str, value: &Value) {
        if let Some(policy) = crate::tools::registry::evidence::for_tool(name) {
            self.result(&policy.computed(value));
        }
    }
    fn collect(&mut self, value: &Value, unit: &ChatUnit, columns: &HashMap<String,ChatUnit>) {
        match value {
            Value::Number(n) => self.evidence(&n.to_string(), unit),
            Value::String(s) => {
                if opaque_identifier(s) { return; }
                self.evidence(s, &ChatUnit::Plain);
                if let Some(d) = date(&normalise(s)) { self.text(d.split('T').next().unwrap_or(&d)); }
            }
            Value::Array(a) => for v in a { self.collect(v, unit, columns); },
            Value::Object(m) => {
                let own_unit = m.get("unit").and_then(Value::as_str).map(ChatUnit::parse).unwrap_or_else(|| unit.clone());
                let mut columns = columns.clone();
                for c in m.get("columns").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(key) = c["key"].as_str() {
                        columns.insert(key.into(), ChatUnit::parse(c["unit"].as_str().unwrap_or_else(|| if c["datatype"] == "percent" { "%" } else { "" })));
                    }
                }
                for (key,v) in m {
                    // Column definitions supply units above, never evidence values.
                    // Opaque IDs cannot authorise claims, and counters stay unit-free.
                    if (key == "unit" && v.is_string()) || (key == "columns" && v.is_array()) || matches!(key.as_str(), "_request" | "id" | "vault" | "cursor" | "next" | "hash" | "sha256" | "checksum" | "url" | "href" | "source" | "target" | "from" | "to" | "path") || (key == "evidence" && m.get("view").and_then(Value::as_str) == Some("metric")) || key == "next_cursor" || key.ends_with("_id") || key.ends_with("_ids") || key.ends_with("_cursor") || key.ends_with("_hash") { continue; }
                    let unit = if matches!(key.as_str(), "count" | "distinct" | "n" | "n_entities" | "total_rows" | "limit" | "offset") { ChatUnit::Plain }
                        else if key == "delta_pct" { ChatUnit::Percent }
                        else { columns.get(key).cloned().unwrap_or_else(|| own_unit.clone()) };
                    self.collect(v, &unit, &columns);
                }
            }
            _ => (),
        }
    }
    fn number_spans(&self, text: &str) -> Vec<(std::ops::Range<usize>, bool)> {
        let units: Vec<_> = self.numbers.iter().map(|n| n.unit.clone()).collect();
        let mut checked: Vec<_> = chat_claims(text, &ChatUnit::Plain, &units).into_iter().map(|(span, shown)| {
            let supported = shown.map(|shown| self.numbers.iter().any(|n| n.supports(&shown)))
                .unwrap_or_else(|| self.exact.contains(&text[span.clone()]));
            (span, supported)
        }).collect();
        // If either end fails, also mask the end relying on the rejected trailing unit.
        let mut start = 0;
        while start < checked.len() {
            let mut end = start;
            while end + 1 < checked.len() && coordinated(&text[checked[end].0.end..checked[end + 1].0.start]) { end += 1; }
            if end > start && checked[start..=end].iter().any(|(_, ok)| !ok) {
                for (_, ok) in &mut checked[start..=end] { *ok = false; }
            }
            start = end + 1;
        }
        checked
    }
    fn checked_tree(&self, raw: &str, mark: bool) -> AppResult<Vec<crate::chat::tree::ChatNode>> {
        use crate::chat::tree::{self, ChatNode};
        static CONTROLS: OnceLock<Regex> = OnceLock::new();
        let controls = CONTROLS.get_or_init(|| Regex::new(r"\p{Cf}").unwrap());
        let data = serde_json::json!({"sources":self.sources});
        // Resolve independently, so a broken placeholder cannot erase useful prose.
        static PLACEHOLDER: OnceLock<Regex> = OnceLock::new();
        let placeholder = PLACEHOLDER.get_or_init(|| Regex::new(r"\{\{.*?(?:\}\}|$)").unwrap());
        let mut changed = false;
        let mut tree = tree::parse(raw);
        let mut units = Vec::new();
        let mut current = Vec::new();
        tree_units(&mut tree, &mut current, &mut units);
        units.push(current);
        for mut unit in units {
            for text in &mut unit {
                let clean = controls.replace_all(text, "");
                **text = placeholder.replace_all(&clean, |c: &regex::Captures<'_>| {
                    match interpolate(&c[0], &data, "assistant reply") {
                        Ok((text,_)) => text,
                        Err(_) => { changed = true; UNVERIFIED.into() }
                    }
                }).into_owned();
            }
            let (text, origins) = unit_text(&unit);
            let mut edits: HashMap<usize, Vec<(std::ops::Range<usize>, String)>> = HashMap::new();
            let mut rejected: Vec<_> = self.number_spans(&text).into_iter().filter_map(|(r,ok)| (!ok).then_some(r)).collect();
            // Includes placeholders introduced by entity decoding or NFKC.
            for token in ["{{", "}}"] {
                rejected.extend(text.match_indices(token).map(|(start,_)| start..start + token.len()));
            }
            if !rejected.is_empty() && !mark { return Err(bad("unsupported number or unresolved placeholder in assistant reply")); }
            for range in rejected {
                changed = true;
                let mut pieces: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
                for (i,r) in &origins[range] {
                    if let Some((last, joined)) = pieces.last_mut() && last == i { joined.end = r.end; }
                    else { pieces.push((*i,r.clone())); }
                }
                for (part,(i,r)) in pieces.into_iter().enumerate() {
                    edits.entry(i).or_default().push((r, if part == 0 { UNVERIFIED.into() } else { String::new() }));
                }
            }
            for (i,mut replacements) in edits {
                replacements.sort_by_key(|(r,_)| std::cmp::Reverse(r.start));
                // Placeholder delimiters can overlap a numeric span. Merge before editing.
                let mut merged: Vec<(std::ops::Range<usize>, String)> = Vec::new();
                for (r, value) in replacements {
                    if let Some((last, _)) = merged.last_mut() && r.end > last.start {
                        last.start = last.start.min(r.start);
                    } else { merged.push((r, value)); }
                }
                for (r,value) in merged { unit[i].replace_range(r, &value); }
            }
        }
        if changed && !mark { return Err(bad("unresolved placeholder in assistant reply")); }
        if changed { tree.push(ChatNode::Paragraph { children: vec![ChatNode::Text { text: NUMBER_NOTE.into() }] }); }
        Ok(tree)
    }
    #[cfg(test)]
    pub(crate) fn check(&self, raw: &str) -> AppResult<String> {
        self.checked_tree(raw, false).map(|_| raw.into())
    }
    pub(crate) fn safe_tree(&self, raw: &str) -> Vec<crate::chat::tree::ChatNode> {
        self.checked_tree(raw, true).expect("marking text nodes is infallible")
    }
    #[cfg(test)]
    fn safe(&self, raw: &str) -> String { crate::chat::tree::visible_text(&self.safe_tree(raw)) }

}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Snapshot of Allowed::check at origin/main 04ee4397. The scanner and string
    // permissions are unchanged, so both rules run the same inputs below.
    fn main_check(allowed: &Allowed, raw: &str, substitutions: &[(std::ops::Range<usize>, String)], path: &str) -> AppResult<()> {
        let text = normalise(raw);
        let numeric = spans(&text);
        let bindings: Vec<_> = substitutions.iter().map(|(range, value)| {
            (normalise(&raw[..range.start]).len()..normalise(&raw[..range.end]).len(), normalise(value))
        }).collect();
        let mut server_ranges: HashMap<std::ops::Range<usize>, HashSet<usize>> = HashMap::new();
        for (source, server) in allowed.strings.iter().enumerate() {
            let server = normalise(server);
            let server_spans = spans(&server);
            for phrase in phrases(&server) {
                for (source_start, _) in server.match_indices(&phrase) {
                    for (start, _) in text.match_indices(&phrase) {
                        let end = start + phrase.len();
                        if text[..start].chars().next_back().is_some_and(token) || following_token(&text[end..]) { continue; }
                        for span in &server_spans {
                            if span.start >= source_start && span.end <= source_start + phrase.len() {
                                let range = start + span.start - source_start..start + span.end - source_start;
                                server_ranges.entry(range).or_default().insert(source);
                            }
                        }
                    }
                }
            }
        }
        for span in &numeric {
            let overlaps: Vec<_> = bindings.iter().filter(|(range, _)| range.start < span.end && span.start < range.end).collect();
            let authorised = if !overlaps.is_empty() {
                // A binding cannot borrow even one connector from prose or another binding.
                overlaps.len() == 1 && overlaps[0].0 == *span && overlaps[0].1 == text[span.clone()]
            } else if iso_span(&text[span.clone()]).is_some() {
                !text[..span.start].chars().next_back().is_some_and(token)
                    && !following_token(&text[span.end..])
                    && iso_span(&text[span.clone()]).is_some_and(|dates| dates.iter().all(|d| allowed.dates.contains(d)))
            } else {
                server_ranges.get(span).is_some_and(|sources| sources.len() == 1)
            };
            if !authorised {
                return Err(bad(format!("unsupported raw digits or combined numeric span in {path}: {raw:?}; use one bound placeholder for the complete formatted number. Raw digits require one verbatim letter-bearing phrase in a server string predating the turn, or a matching applied ISO date or bucket label. Number words are unchecked.")));
            }
        }
        Ok(())
    }
    #[test]
    fn boundary_probe_matches_main_except_custom_units_at_clause_boundaries() {
        // Brackets mark the exact server binding. Both verdicts are committed
        // alongside every input, including the pre-existing gaps owned by #226.
        for row in include_str!("prose-boundaries.tsv").lines().skip(1) {
            let columns: Vec<_> = row.split('\t').collect();
            assert_eq!(columns.len(), 3, "{row}");
            let mut raw = String::new();
            let mut subs = Vec::new();
            let mut rest = columns[0];
            while let Some((before, tail)) = rest.split_once('[') {
                raw.push_str(before);
                let (value, tail) = tail.split_once(']').unwrap();
                let start = raw.len();
                raw.push_str(value);
                subs.push((start..raw.len(), value.to_string()));
                rest = tail;
            }
            raw.push_str(rest);
            let raw = raw.replace("\\n", "\n");
            let allowed = Allowed::default();
            assert_eq!(main_check(&allowed, &raw, &subs, "probe").is_ok(), columns[1] == "Pass", "main: {row}");
            assert_eq!(allowed.check(&raw, &subs, "probe").is_ok(), columns[2] == "Pass", "PR: {row}");
        }
        for value in ["12", "-12", "+12"] {
          for suffix in ["st", "nd", "rd", "th", "st.", "nd,", "rd)", "th item"] {
            let raw = format!("{value}{suffix}");
            let subs = [(0..value.len(), value.into())];
            assert!(main_check(&Allowed::default(), &raw, &subs, "probe").is_ok());
            assert!(Allowed::default().check(&raw, &subs, "probe").is_ok(), "{raw}");
          }
        }
        for suffix in ["/s", "%", "²", "³", "^2", "/per second", " / s", " %", "·s", "*s"] {
            let raw = format!("5 m{suffix}");
            assert!(Allowed::default().check(&raw, &[(0..3, "5 m".into())], "probe").is_err(), "{raw}");
            assert!(Allowed::default().check(&raw, &[(0..raw.len(), raw.clone())], "probe").is_ok(), "complete: {raw}");
        }
    }
    #[test]
    fn catalogue_without_diagrams_or_bindings_is_identical_to_main() {
        let allowed = Allowed::from_data(&data());
        for raw in ["Monthly sales", "Twelve items", "ISO 27001", "Scope 3", "Scope 3 thousand", "Scope 3 k", "As at 2026-01-01", "12", "12th", "12-item", "5 m/s", "5 m%", "x12", "Revenue 12%", "2026-02-30"] {
            assert_eq!(allowed.check(raw, &[], "text").is_ok(), main_check(&allowed, raw, &[], "text").is_ok(), "{raw}");
            for field in ["title", "summary", "text", "label", "headline", "note", "reason", "caption", "legend", "why"] {
                assert_eq!(resolve(&json!({field: raw}), &data()).is_ok(), main_check(&allowed, raw, &[], field).is_ok(), "{field}: {raw}");
            }
        }
    }
    #[test]
    fn bound_values_are_complete_tokens_including_their_custom_units() {
        let placeholder = "{{/sources/graph/diagram/nodes/0/metrics/0/value}}";
        for (value, unit, suffix) in [(5, Some("m"), "illion customers"), (5, Some("m"), "m"), (42, Some("orders"), "today"), (5, Some("m"), " long"), (42, Some("orders"), " today"), (5, Some("m"), " million"), (5, Some("m"), " squared"), (5, Some("m"), " per second")] {
            let data = json!({"sources":{"graph":{"view":"diagram","diagram":{"nodes":[{"metrics":[{"value":value,"unit":unit}]}]}}}});
            assert!(resolve(&json!({"title":format!("{placeholder}{suffix}")}), &data).is_err(), "{value} {unit:?} + {suffix}");
            for prefix in ["customers", "İ", "客", "Ｘ"] {
                assert!(resolve(&json!({"title":format!("{prefix}{placeholder}")}), &data).is_err(), "{prefix}");
            }
        }
        let data = json!({"sources":{"graph":{"view":"diagram","diagram":{"nodes":[{"metrics":[{"value":42,"unit":"orders"}]}]}}}});
        assert_eq!(resolve(&json!({"cells":[{"text":placeholder}], "items":[{"text":placeholder}]}), &data).unwrap(), json!({"cells":[{"text":"42 orders"}], "items":[{"text":"42 orders"}]}));
        for (raw, expected) in [(format!("{placeholder}."), "42 orders."), (format!("{placeholder}, next"), "42 orders, next"), (format!("({placeholder})"), "(42 orders)")] {
            assert_eq!(resolve(&json!({"title":raw}), &data).unwrap()["title"], expected);
        }
    }
    #[test]
    fn diagram_bindings_keep_metric_units_and_refuse_layout_coordinates() {
        let data = json!({"sources":{"graph":{"view":"diagram","diagram":{"total_nodes":2,"nodes":[{"x":80,"metrics":[{"value":42,"unit":"AUD","n_entities":1}]}]}}}});
        assert_eq!(bound("/sources/graph/diagram/nodes/0/metrics/0/value", &data).unwrap(), "42.00 AUD");
        assert_eq!(bound("/sources/graph/diagram/total_nodes", &data).unwrap(), "2");
        let custom = json!({"sources":{"graph":{"view":"diagram","diagram":{"nodes":[{"metrics":[{"value":42,"unit":"orders"}]}]}}}});
        assert_eq!(resolve(&json!({"title":"Count {{/sources/graph/diagram/nodes/0/metrics/0/value}}"}), &custom).unwrap()["title"], "Count 42 orders");
        assert!(resolve(&json!({"title":"Count {{/sources/graph/diagram/nodes/0/metrics/0/value}}999"}), &custom).is_err());
        assert!(bound("/sources/graph/diagram/nodes/0/x", &data).is_err());
    }

    #[test]
    fn computed_write_receipts_verify_but_request_only_counts_do_not() {
        for (tool, result, reply) in [
            ("document_import_rows", json!({"created":6240,"rows":6241,"bad_count":1,"_request":{"limit":999777}}), "Created 6240 entities from 6241 rows with 1 bad cell"),
            ("document_upload", json!({"filename":"999777.csv","size_bytes":6240,"chunk_count":3,"revision":1}), "Uploaded 6240 bytes in 3 chunks"),
            ("ontology_automap", json!({"mode":"profile_only","profiles":[{"records":6240,"fields":[{"distinct_count":3}]}],"_request":{"limit":999777}}), "Profiled 6240 records with 3 distinct values"),
        ] {
            let mut prose = ChatProse::default();
            prose.tool_result(tool, &result);
            assert!(prose.check(reply).is_ok(), "{tool}: {}", prose.safe(reply));
            assert!(prose.check("Processed 999777 rows").is_err(), "{tool}");
        }
    }

    #[test]
    fn coordinated_numbers_share_trailing_units_and_leading_currency() {
        let mut prose = ChatProse::default();
        prose.result(&json!([{"value":3,"unit":"kg"},{"value":4,"unit":"kg"},{"value":5,"unit":"kg"}]));
        for raw in ["3 and 5 kg", "3, 4 and 5 kg", "3, 4, and 5 kg", "between 3 and 5 kg", "3 to 5 kg", "from 3 up to 5 kg", "3-5 kg", "3 - 5 kg", "3–5 kg", "3 or 5 kg"] {
            assert!(prose.check(raw).is_ok(), "{raw}: {}", prose.safe(raw));
        }
        for raw in ["between 3 and 999 kg", "3, 999 and 5 kg", "There were 3. And 5 kg", "3; 5 kg", "3 m and 5 kg", "3 kg and 5 m", "3 apples and 5 kg"] {
            assert!(prose.check(raw).is_err(), "{raw}");
        }
        let mut money = ChatProse::default();
        money.result(&json!([{"value":3,"unit":"AUD"},{"value":5,"unit":"AUD"},{"value":14,"unit":"AUD"},{"value":15.1,"unit":"AUD"}]));
        for raw in ["$3 to 5", "AUD 14 to 15.10", "$3 and 5", "AUD 14-15.10"] { assert!(money.check(raw).is_ok(), "{raw}: {}", money.safe(raw)); }
        for raw in ["$3 to 999", "AUD 14. Then 15.10", "$3 to 5 kg", "$3 to 5 metres", "$3 to 5 USD"] { assert!(money.check(raw).is_err(), "{raw}"); }
    }

    #[test]
    fn argument_digits_do_not_damage_computed_values_or_names() {
        let mut area = ChatProse::default();
        area.tool_result("metric_eval", &json!({"value":1000,"unit":"m²","_request":{"top_n":2}}));
        assert_eq!(area.safe("1000 m²"), "1000 m²");
        let mut name = ChatProse::default();
        name.tool_result("entities_get", &json!({"id":"entity:a3b","name":"Store 3","_request":{"entity_id":"entity:a3b"}}));
        assert_eq!(name.safe("Store 3"), "Store 3");
    }

    #[test]
    fn opaque_identifiers_never_supply_numeric_evidence() {
        let mut prose = ChatProse::default();
        prose.tool_result("entities_get", &json!({"id":"entity:a3b","next":"cursor987654", "sha256":"999777abcdef999777abcdef999777abcdef999777abcdef999777abcdef999777ab", "values":["entity:x876543", "01ARZ3NDEKTSV4RRFFQ69G5FAV"], "summary":"Read (entity:x876543) for details"}));
        for raw in ["3", "987654", "876543", "999777", "4", "69", "5"] { assert!(prose.check(raw).is_err(), "{raw}"); }
    }

    #[test]
    fn unit_digits_never_become_independent_evidence() {
        for unit in ["m²", "m³", "CO₂", "km/h²", "per 100 g", "m^2", "CO2"] {
            for evidence in [json!({"value":1000,"unit":unit}), json!(format!("1000 {unit}"))] {
                let mut prose = ChatProse::default();
                prose.result(&evidence);
                assert!(prose.check(&format!("1000 {unit}")).is_ok(), "{unit}");
                assert!(prose.check("1000").is_err(), "unit cannot be dropped: {unit}");
                assert!(prose.check("1000 incorrect²").is_err(), "unit cannot change: {unit}");
                assert_eq!(prose.safe(&format!("999 {unit}")), format!("{UNVERIFIED}{NUMBER_NOTE}"), "one claim: {unit}");
                for number in ["2", "3", "100"] {
                    assert!(prose.check(&format!("There are {number} items")).is_err(), "{unit}: {number}");
                }
            }
        }
    }

    #[test]
    fn echoed_arguments_never_authorise_numbers_for_any_tool() {
        for (tool, result) in [
            ("show_view", json!({"pagination":{"reach":{"depth":2,"kinds":["sample999777"]},"total":0},"text":"No results for sample999777"})),
            ("metric_eval", json!({"filters":[{"value":999777}],"value":null,"n_entities":0,"warnings":["No score above 999777","No score above 999,777","No score above 999.777k"]})),
            ("scenario_run", json!({"changes":["Scale score by 999777"],"n_changed":0,"scenario":[]})),
            ("search", json!({"query":"sample999777","limit":876543,"results":[],"error":"No sample999777 records"})),
            ("unknown_future_tool", json!({"echo":{"amount":999777}})),
        ] {
            let mut prose = ChatProse::default();
            prose.tool_result(tool, &result);
            for text in ["There are 999777 affected items", "There are 999,777 items", "There are 999.777k items", "There are 876543 items"] {
                assert!(prose.check(text).is_err(), "{tool}: {text}: {result}");
                assert!(prose.safe(text).contains(UNVERIFIED));
            }
        }
    }

    #[test]
    fn shared_range_units_require_both_values_with_the_shared_unit() {
        for unit in ["AUD", "m²", "CO₂", "km/h²", "per 100 g"] {
            let mut prose = ChatProse::default();
            prose.result(&json!([{"value":14,"unit":unit},{"value":15.1,"unit":unit}]));
            let good = format!("Total goes from 14 to 15.10 {unit}");
            assert_eq!(prose.safe(&good), good, "{unit}");
            assert!(prose.check("14").is_err(), "a range does not license dropping the unit");
            for raw in [format!("999777 to 15.10 {unit}"), format!("14 to 999777 {unit}")] {
                assert!(prose.check(&raw).is_err(), "{raw}");
                let safe = prose.safe(&raw);
                assert!(!safe.contains("14") && !safe.contains("15.10") && !safe.contains("999777"), "{safe}");
            }
            let mut count = ChatProse::default();
            count.result(&json!([14,{"value":15.1,"unit":unit}]));
            assert!(count.check(&good).is_err(), "a unit-free count is not a value in {unit}");
        }
    }

    #[test]
    fn request_field_names_do_not_taint_independent_computed_columns() {
        for key in ["top_n", "filters", "query", "reach", "warnings", "unit", "columns"] {
            let mut prose = ChatProse::default();
            prose.tool_result("show_view", &json!({"data":{"sources":{"sales":{
                "parameters":{"top_n":2},"rows":[{key:1000}]
            }}}}));
            assert!(prose.check("1000").is_ok(), "{key}");
            assert!(prose.check("2").is_err(), "{key}");
        }
    }

    #[test]
    fn column_metadata_supplies_units_without_supplying_numbers() {
        let mut prose = ChatProse::default();
        prose.tool_result("show_view", &json!({"data":{"sources":{"sales":{
            "columns":[{"key":"amount_2026","label":"Cost 999777","datatype":"money","unit":"AUD"}],
            "totals":{"amount_2026":{"sum":31}}
        }}}}));
        assert!(prose.check("The total is $31.00").is_ok());
        for raw in ["2026", "999777", "31"] { assert!(prose.check(raw).is_err(), "{raw}"); }
    }

    #[test]
    fn computed_numbers_still_verify_when_the_arguments_contain_them() {
        for value in [1000, 999777] {
            let mut prose = ChatProse::default();
            prose.tool_result("metric_eval", &json!({"filters":[{"property":"score","op":"eq","value":value}],"value":value,"n_entities":1}));
            assert!(prose.check(&format!("The result is {value}")).is_ok());
        }
        let mut scenario = ChatProse::default();
        scenario.tool_result("scenario_run", &json!({"changes":["Price up 10%"],"scenario":[{"value":110,"unit":"AUD","delta_pct":10}]}));
        assert!(scenario.check("The result increased 10%").is_ok());
        let mut rows = ChatProse::default();
        rows.tool_result("show_view", &json!({"parameters":{"top_n":5},"total_rows":5,"rows":[{}, {}, {}, {}, {}]}));
        assert!(rows.check("The top 5 rows").is_ok());
    }

    #[test]
    fn paging_metadata_cannot_authorise_prose_numbers() {
        let mut prose = ChatProse::default();
        prose.result(&json!({"view":"metric","metric":{"value":12},"evidence":{"args":{"top_n":987654,"filters":[{"value":876543}]},"next_cursor":"opaque765432"}}));
        assert!(prose.check("The result is 12").is_ok());
        for text in ["The result is 987654", "The result is 876543", "The result is 765432"] {
            assert!(prose.check(text).is_err(), "{text}");
        }
    }

    #[test]
    fn chat_rounds_to_displayed_precision_and_scale() {
        for (evidence, good, bad) in [
            (json!({"value":664.475,"unit":"AUD"}), "$664.48", "$664.47"),
            (json!({"value":34.578125,"unit":"%"}), "34.58%", "34.57%"),
            (json!({"value":615548,"unit":"AUD"}), "$615.5k", "$616.5k"),
            (json!({"value":615548000,"unit":"AUD"}), "$615.5M", "$616.5M"),
            (json!({"value":615548000}), "615.5M", "616.5M"),
            (json!({"value":615548000000_i64,"unit":"AUD"}), "$615.5 billion", "$616.5 billion"),
            (json!({"value":615548,"unit":"AUD"}), "AUD 615.5 thousand", "AUD 616.5 thousand"),
            (json!({"value":615548000,"unit":"AUD"}), "615.5 million AUD", "616.5 million AUD"),
        ] {
            let mut prose = ChatProse::default(); prose.result(&evidence);
            assert!(prose.check(good).is_ok(), "{good} from {evidence}");
            assert!(prose.check(bad).is_err(), "{bad} from {evidence}");
        }
    }
    #[test]
    fn chat_checks_visible_markdown_and_decoded_entities() {
        let mut prose = ChatProse::default(); prose.result(&json!([1,2]));
        for raw in ["1**2**", "1&#50;", "&#123;&#123;sources.sales.value&#125;&#125;", "&lbrace;&lbrace;sources.sales.value&rbrace;&rbrace;"] {
            assert!(prose.check(raw).is_err(), "{raw}");
        }
        prose.result(&json!(12));
        assert_eq!(prose.check("**Count:** 1**2** &amp; done").unwrap(), "**Count:** 1**2** &amp; done");
    }
    #[test]
    fn chat_preserves_numeric_units_and_scaled_values() {
        let mut prose = ChatProse::default(); prose.result(&json!(12));
        assert!(prose.check("12 records").is_ok());
        for raw in ["$12", "$12k", "12%", "12 percentage", "12 thousand", "12M", "12 billion"] {
            assert!(prose.check(raw).is_err(), "{raw}");
        }
        for evidence in [json!({"value":12,"unit":"%"}), json!({"value":0.12,"unit":"ratio"})] {
            let mut prose = ChatProse::default(); prose.result(&evidence);
            for shown in ["12%", "12 percent", "12 percentage"] { assert!(prose.check(shown).is_ok(), "{evidence}: {shown}"); }
            assert!(prose.check("$12").is_err());
        }
        let mut prose = ChatProse::default(); prose.result(&json!({"value":12000,"unit":"AUD"}));
        assert!(prose.check("$12k").is_ok());
        assert!(prose.check("12%").is_err());
        assert!(prose.check("USD 12k").is_err());
        assert!(prose.check("$12k USD").is_err());
        assert!(prose.check("$12k AUD").is_ok());
        let mut prose = ChatProse::default(); prose.text("9007199254740992");
        assert!(prose.check("9007199254740993").is_err());
    }
    #[test]
    fn chat_rounding_never_erases_all_significant_digits() {
        for (value, good, bad) in [(0.49, "$0.49", "$0"), (0.0049, "$0.005", "$0.00"), (-0.49, "-$0.49", "$0")] {
            let mut prose = ChatProse::default(); prose.result(&json!({"value":value,"unit":"AUD"}));
            assert!(prose.check(good).is_ok());
            assert!(prose.check(bad).is_err(), "{value} must not support {bad}");
        }
        let mut zero = ChatProse::default(); zero.result(&json!({"value":0,"unit":"AUD"}));
        assert!(zero.check("$0.00").is_ok());
    }
    #[test]
    fn chat_decimal_matching_never_silently_discards_digits() {
        let mut prose = ChatProse::default(); prose.result(&json!(12));
        assert!(prose.check("12.00000000000000000000000000001").is_err());
        assert!(prose.check("12.00000000000000000000000000000").is_ok());
    }
    #[test]
    fn chat_matches_arbitrary_evidence_units_and_common_forms() {
        for (unit, shown) in [("kg", "12 kg"), ("kg", "12 kilograms"), ("kg", "12kg"), ("metre", "12 m"), ("µm", "12 µm"), ("m²", "12 m²"), ("ratio", "12 ratio"), ("ratio", "12 ratios"), ("widget", "12 widgets"), ("WIDGETS", "12 widget"), ("litre", "12 L"), ("boxes shipped", "12 boxes shipped")] {
            let mut prose = ChatProse::default(); prose.result(&json!({"value":12,"unit":unit}));
            assert!(prose.check(shown).is_ok(), "{unit}: {shown}");
            assert!(prose.check("12%").is_err());
            assert!(prose.check("$12").is_err());
        }
    }
    #[test]
    fn chat_table_cells_are_independent_visible_units() {
        let mut prose = ChatProse::default(); prose.result(&json!([12,34]));
        let reply = "| Left | Right |\n| --- | --- |\n| 1**2** | `34` |";
        assert_eq!(prose.check(reply).unwrap(), reply);
        assert!(prose.check("- 12\n\n  34").is_ok());
        assert!(ChatProse::default().safe("Useful **advice** and 999.").contains("Useful advice"));
    }
    #[test]
    fn chat_tree_matches_shared_parity_corpus() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chat_markdown.json");
        let mut fixture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let evidence = fixture["evidence"].clone();
        for case in fixture["cases"].as_array_mut().unwrap() {
            let mut prose = ChatProse::default();
            prose.result(case.get("evidence").unwrap_or(&evidence));
            let tree = prose.safe_tree(case["reply"].as_str().unwrap());
            let text = crate::chat::tree::visible_text(&tree);
            if std::env::var_os("UPDATE_CHAT_MARKDOWN_FIXTURE").is_some() {
                case["tree"] = json!(tree); case["visible_text"] = json!(text);
            } else {
                assert_eq!(case["tree"], json!(tree), "{}", case["name"]);
                assert_eq!(case["visible_text"], text, "{}", case["name"]);
            }
        }
        if std::env::var_os("UPDATE_CHAT_MARKDOWN_FIXTURE").is_some() {
            std::fs::write(path, serde_json::to_string_pretty(&fixture).unwrap() + "\n").unwrap();
        }
    }
    #[test]
    fn image_alt_cannot_authorise_a_different_visible_number() {
        let mut prose = ChatProse::default(); prose.result(&json!(123));
        let tree = prose.safe_tree("Count 12![3](/icon.svg)");
        assert_eq!(crate::chat::tree::visible_text(&tree), format!("Count {UNVERIFIED}{NUMBER_NOTE}"));
        assert!(prose.check("Count 12![3](/icon.svg)").is_err());
    }
    #[test]
    fn autolink_urls_are_not_claims_and_hrefs_are_never_rewritten() {
        use crate::chat::tree::{ChatNode, visible_text};
        let tree = ChatProse::default().safe_tree("<https://example.com/999>");
        assert_eq!(visible_text(&tree), "https://example.com/999");
        assert!(matches!(&tree[0].children()[0], ChatNode::Link { href, autolink: true, .. } if href == "https://example.com/999"));
        let tree = ChatProse::default().safe_tree("[999](https://example.com/999)");
        assert!(matches!(&tree[0].children()[0], ChatNode::Link { href, autolink: false, .. } if href == "https://example.com/999"));
        assert!(visible_text(&tree).starts_with(UNVERIFIED));
    }
    #[test]
    fn only_navigation_autolinks_exempt_their_url_digits() {
        use crate::chat::tree::{ChatNode, visible_text};
        let prose = ChatProse::default();
        for raw in ["Bakery sales <AUD:0.00>.", "<ftp://example.com/999>", "<custom:999>", "www.example.com/999"] {
            let tree = prose.safe_tree(raw);
            assert!(visible_text(&tree).contains(UNVERIFIED), "{raw}");
            assert!(prose.check(raw).is_err(), "{raw}");
            assert!(!serde_json::to_string(&tree).unwrap().contains("\"kind\":\"link\""), "{raw}");
        }
        for raw in ["<http://example.com/999>", "<HTTPS://example.com/999>", "<mailto:person999@example.com>", "<person999@example.com>"] {
            let tree = prose.safe_tree(raw);
            assert!(!visible_text(&tree).contains(UNVERIFIED), "{raw}");
            assert!(matches!(&tree[0].children()[0], ChatNode::Link { autolink: true, .. }), "{raw}");
            assert!(prose.check(&format!("999{raw}999")).is_err(), "adjacent claims: {raw}");
        }
    }
    #[test]
    fn link_titles_and_code_info_are_absent_and_visible_attributes_are_checked() {
        let prose = ChatProse::default();
        for raw in ["[Report](https://example.com \"Bakery sales $0.00\")", "[Report][ref]\n\n[ref]: https://example.com \"Bakery sales $0.00\"", "```Bakery sales $0.00\nUseful code\n```"] {
            let tree = prose.safe_tree(raw);
            let encoded = serde_json::to_string(&tree).unwrap();
            assert!(!encoded.contains("$0.00"), "{raw}");
            assert!(!encoded.contains("\"title\"") && !encoded.contains("\"language\""), "{encoded}");
        }
        for raw in ["# Sales {#999}", "| Sales {#999} |\n| --- |\n| Useful |", "[^999]: Bakery sales $0.00\n\nFootnote [^999]"] {
            let text = prose.safe(raw);
            assert!(!text.contains("999") && !text.contains("0.00"), "{text}");
            assert!(text.contains(UNVERIFIED), "{raw}");
        }
        let tree = prose.safe_tree("999. Useful list item");
        assert!(matches!(tree[0], crate::chat::tree::ChatNode::List { start: Some(1), .. }));
    }
    #[test]
    fn chat_marks_only_bad_spans_even_across_inline_nodes() {
        let mut prose = ChatProse::default(); prose.result(&json!([12,34]));
        assert_eq!(prose.safe("Count 1\u{202e}2"), "Count 12");
        let marked = prose.safe("Keep **12**, [34](https://example.com). Wrong 9**9**9 and `888`. Also &#55;77.");
        assert_eq!(marked.matches("unverified number").count(),3,"{marked}");
        assert!(marked.contains("12")); assert!(marked.contains("34"));
        assert!(!marked.contains("999")); assert!(!marked.contains("888")); assert!(!marked.contains("777"));
        assert_eq!(marked.matches(NUMBER_NOTE).count(),1);
    }
    fn data() -> Value { json!({"sources":{"sales":{
        "rows":[{"amount":1588.0,"quantity":12,"other":1,"next":2,"category":"ISO 27001","name":"Scope 3","date":"2026-01-01"}],
        "columns":[{"key":"amount","datatype":"money","unit":"AUD"},{"key":"quantity","datatype":"integer"},{"key":"category","datatype":"text"},{"key":"date","datatype":"date"}],
        "total_rows":12,"totals":{"quantity":{"sum":49},"amount":{"sum":1588.0,"count":1}},"parameters":{"as_of":"2026-01-01","limit":12},"bucket_column":"date"
    }}}) }
    fn title(s: &str, data: &Value) -> AppResult<Value> { resolve(&json!({"title":s}), data) }
    #[test]
    fn displayed_prose_removes_bidi_and_format_controls() {
        let p = "{{/sources/sales/total_rows}}";
        for control in ['\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}', '\u{200e}', '\u{200f}', '\u{061c}', '\u{200b}'] {
            let raw = format!("Count {control}{p}\u{202c}");
            let spec = resolve(&json!({"elements":{"a":{"props":{"title":raw}}}}), &data()).unwrap();
            assert_eq!(spec["elements"]["a"]["props"]["title"], "Count 12");
            assert_eq!(super::super::text(&spec, &json!({})), "Analysis\n• Count 12\n");
            assert!(title(&format!("{p}{control}3"), &data()).is_err());
        }
        assert_eq!(title("Ｃｏｕｎｔ {{/sources/sales/total_rows}}", &data()).unwrap()["title"], "Count 12");
        // RTL letters remain supported. Explicit bidi marks are deliberately removed.
        for rtl in ["إجمالي المبيعات", "סך המכירות"] {
            assert_eq!(title(rtl, &data()).unwrap()["title"], display(rtl));
        }
    }
    #[test]
    fn spaced_modifiers_cannot_change_bound_counts() {
        let p = "{{/sources/sales/total_rows}}";
        for space in [" ", "   ", "\t", "\n\r\n", "\u{00a0}", "\u{2009}\u{202f}", "\u{3000}"] {
            for modifier in ["−", "+", "%", "‰", "$", "€", "USD", "EUR", "ZAR", "BHD", "XAU"] {
                for raw in [format!("{modifier}{space}{p}"), format!("{p}{space}{modifier}")] {
                    assert!(title(&raw, &data()).is_err(), "{raw:?}");
                }
            }
        }
        let mut data = data();
        data["sources"]["sales"]["columns"].as_array_mut().unwrap().push(json!({"key":"share","datatype":"percent","unit":"%"}));
        data["sources"]["sales"]["rows"][0]["share"] = json!(12);
        for (raw, expected) in [
            ("Share {{/sources/sales/rows/0/share}}", "Share 12%"),
            ("Total {{/sources/sales/rows/0/amount}}", "Total 1588.00 AUD"),
            ("Count {{/sources/sales/total_rows}} per cent", "Count 12 per cent"),
        ] { assert_eq!(title(raw, &data).unwrap()["title"], expected); }
    }
    #[test]
    fn identifier_numeric_spans_must_attach_to_letters() {
        for name in ["revenue_999", "revenue_1e2", "revenue_999e", "name_999", "e_999", "999999", "12_sales", "sales_999999", "sales_12_34", "12q", "_12sales", "sales_１２", "sales\u{202e}12", "sales-12", "sales 12"] {
            assert!(identifier(name).is_err(), "{name:?}");
        }
        for name in ["sales", "sales_q3", "revenue_q3", "e3", "q12", "sales_q12x", "_sales", "amount"] {
            assert!(identifier(name).is_ok(), "{name}");
        }
    }
    #[test]
    fn final_placeholder_boundaries_refuse_numeric_synthesis() {
        let p = "{{/sources/sales/total_rows}}";
        for raw in [format!("Orders {p}{p}"), format!("{p}{{{{/sources/sales/rows/0/date}}}}"), format!("{p}%"), format!("${p}"), format!("{p}e{p}"), format!("e{p}"), format!("{p}E"), format!("{p}１"), format!("{p}\u{200b}{p}"), format!("{p}\u{200b}%"), format!("€{p}"), format!("{p}\u{200b}E")] {
            assert!(title(&raw, &data()).is_err(), "{raw}");
        }
        for raw in [format!("Orders ({p}), then {p} records"), format!("{p} records"), format!("Count {p}"), format!("{p}\u{200b} records"), format!("{p}."), format!("{p}, then"), format!("{p}. Next"), format!("{p}.Next")] {
            assert!(title(&raw, &data()).is_ok(), "{raw}");
        }
    }
    #[test]
    fn whole_spans_refuse_gluing_across_every_source() {
        let p = "{{/sources/sales/total_rows}}";
        let mut data = data();
        data["sources"]["sales"]["rows"][0]["name"] = json!("Orders 12");
        data["sources"]["sales"]["rows"][0]["category"] = json!("345 deliveries");
        for raw in [
            format!("−{p}"), format!("－{p}"), format!("+{p}"), format!("{p}%"),
            format!("{p}.5"), format!("{p},5"), format!("{p}e3"), format!("A${p}"), format!("{p} {p}"), format!("{p}  {p}"), format!("{p}\n\n{p}"),
            format!("{p}\u{034f}%"), format!("{p}\u{034f}{p}"), format!("{p}\u{fe0f}5"), format!("{p}\u{0301}5"),
            format!("Orders {p},345 deliveries"), format!("{p}‰"), format!("{p}'345"),
            format!("USD{p}"), format!("{p} AUD"), format!("{p}/2"), format!("{p}½"),
            "Orders 12,345 deliveries".into(), "Orders 12 345 deliveries".into(),
            "Orders 1\u{034f}2,345 deliveries".into(), "Orders 12_345 deliveries".into(),
        ] {
            assert!(title(&raw, &data).is_err(), "{raw}");
        }
        for raw in [format!("{p} records and {p} deliveries"), "Orders 12 and 345 deliveries".into()] {
            assert!(title(&raw, &data).is_ok(), "{raw}");
        }
        // Matching digits alone cannot borrow a server phrase over a placeholder.
        assert!(title(&format!("Orders +{p}"), &data).is_err());
        data["sources"]["sales"]["rows"][0]["name"] = json!("Orders 12,345");
        assert!(title("Orders 12,345", &data).is_ok());
        assert!(title("Orders 12", &data).is_err());
        assert!(title("Orders +12,345", &data).is_err());
    }
    #[test]
    fn greedy_span_extraction_and_normalisation() {
        let text = normalise("−１２\u{034f}３ USD +٤.٥e3 12 345 A$6 ½ 7‰ 8_000");
        assert_eq!(text, "-123 usd +4.5e3 12 345 a$6 1⁄2 7‰ 8_000");
        let values: Vec<_> = spans(&text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(values, ["-123 usd +4.5e3 12 345", "$6 1⁄2 7‰ 8_000"]);
        for sign in ['\u{2212}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}', '\u{fe63}', '\u{ff0d}'] {
            assert_eq!(normalise(&format!("{sign}12")), "-12");
        }
        assert_eq!(normalise("1\u{0301}2\u{20dd}3\u{200b}4"), "1234");
        assert_eq!(normalise("  a \t b  "), " a b ");
    }
    #[test]
    fn bucket_phrases_and_complete_formatted_bindings_still_pass() {
        let mut data = data();
        data["sources"]["sales"]["rows"][0]["date"] = json!("Q3 2026");
        for raw in ["ISO 27001", "Scope 3", "Q3 2026", "Total {{/sources/sales/rows/0/amount}}"] {
            assert!(title(raw, &data).is_ok(), "{raw}");
        }
        assert!(title("Q3 2026,345", &data).is_err());
        assert!(title("Scope 3.", &data).is_ok());
        assert!(title("Scope 3.Next", &data).is_ok());
        data["sources"]["sales"]["columns"].as_array_mut().unwrap().push(json!({"key":"share","datatype":"percent","unit":"%"}));
        data["sources"]["sales"]["rows"][0]["share"] = json!(12.5);
        assert_eq!(title("Share {{/sources/sales/rows/0/share}}", &data).unwrap()["title"], "Share 12.5%");
        assert!(title("Share {{/sources/sales/rows/0/share}}%", &data).is_err());
        for suffix in ["%", "‰", "+", "-", "e3", ".5", ",5", "/2"] {
            assert!(title(&format!("Total {{{{/sources/sales/rows/0/amount}}}}{suffix}"), &data).is_err(), "{suffix}");
        }
    }
    #[test]
    fn terminal_summary_keeps_prose_values_in_separate_items() {
        let spec = resolve(&json!({"elements":{
            "a":{"props":{"text":"{{/sources/sales/total_rows}}"}},
            "b":{"props":{"text":"{{/sources/sales/total_rows}}"}}
        }}), &data()).unwrap();
        let text = super::super::text(&spec, &json!({}));
        assert_eq!(text, "Analysis\n• 12\n• 12\n");
        let normalised = normalise(&text);
        assert_eq!(spans(&normalised).len(), 2);
    }
    #[test]
    fn bare_numeric_server_strings_never_authorise_digits() {
        let mut data = data();
        data["sources"]["sales"]["rows"][0]["name"] = json!("12%");
        data["sources"]["sales"]["rows"][0]["category"] = json!("12");
        for raw in ["Revenue rose 12%", "Revenue rose 12", "12%", "12"] {
            assert!(title(raw, &data).is_err(), "{raw}");
        }
        data["sources"]["sales"]["rows"][0]["name"] = json!("Truck 7");
        assert!(title("Truck 7 deliveries", &data).is_ok());
        assert!(title("7 deliveries", &data).is_err());
        data["sources"]["sales"]["rows"][0]["name"] = json!("Report 2026-02-01");
        assert!(title("Report 2026-02-01", &data).is_err());
        // Words in separate strings cannot combine to create a permission.
        data["sources"]["sales"]["rows"][0]["name"] = json!("Scope");
        data["sources"]["sales"]["rows"][0]["category"] = json!("3");
        assert!(title("Scope 3", &data).is_err());
    }
    #[test]
    fn realistic_business_titles_have_zero_false_rejections() {
        let mut data = data();
        for name in ["Q3", "B2B", "COVID-19"] { data["sources"]["sales"]["rows"].as_array_mut().unwrap().push(json!({"category":name})); }
        let titles = ["Monthly revenue by store", "Weekly cash flow", "Revenue versus budget", "First Nations supplier spend", "Twice-yearly stocktake costs", "Sales score by branch", "Stage IV investment pipeline", "One-off implementation costs", "Customer churn and retention", "Quarter-on-quarter gross margin", "Year-to-date operating expenses", "Accounts receivable ageing", "Top-performing products", "Cash conversion cycle", "Operating costs by department", "Sales total {{/sources/sales/totals/amount/sum}}", "Units sold {{/sources/sales/totals/quantity/sum}}", "Records reviewed {{/sources/sales/total_rows}}", "As at {{sources.sales.rows[0].date}}", "ISO 27001 audit readiness", "Scope 3 emissions by supplier", "Q3 revenue by region", "B2B pipeline coverage", "COVID-19 absence costs", "As at 2026-01-01"];
        let rejected: Vec<_> = titles.iter().filter(|t| title(t, &data).is_err()).collect();
        assert!(rejected.is_empty(), "false rejections out of 25: {rejected:?}");
    }
    #[test]
    fn unrelated_values_and_identifier_bypasses_are_rejected() {
        for raw in ["Sales rose 12%", "Sales rose 1\u{200b}2%", "Sales rose 1\u{2009}2%", "Sales rose ½", "12k", "x12000", "1e6%", "１２", "١٢", "¹²", "⑫", "Top 12", "Value 1.0", "1/2", "Quarter 3", "Year 2026"] {
            let error = title(raw, &data()).unwrap_err();
            assert!(error.message.contains("/title") && error.message.contains("placeholder"), "{raw}: {error:?}");
        }
        for raw in ["twice", "twelve", "dos mil euros", "IV", "Café sales"] { assert!(title(raw, &data()).is_ok()); }
    }
    #[test]
    fn verbatim_tokens_are_scoped_to_resolved_server_strings() {
        let mut data = data();
        for raw in ["ISO 27001 compliance", "Scope 3 emissions", "iso ２７００１ compliance", "Scope\u{2009}3 emissions", "ISO 27001.", "Scope 3."] { assert!(title(raw, &data).is_ok(), "{raw}"); }
        for raw in ["ISO 2700 compliance", "Scope 30 emissions", "Q3 revenue", "xISO 27001", "ISO 27001%", "ISO 27001$", "27,001 compliance", "ISO 27001.5"] { assert!(title(raw, &data).is_err(), "{raw}"); }
        data["sources"]["sales"]["rows"][0]["category"] = json!("Q3");
        assert!(title("Q3 revenue", &data).is_ok());
        assert!(title("ISO 27001 compliance", &data).is_err());
        data["sources"]["sales"]["rows"][0]["name"] = json!("Other");
        data["sources"]["sales"]["headline"] = json!("Scope 3");
        data["sources"]["sales"]["text"] = json!("ISO 27001");
        data["sources"]["sales"]["parameters"]["label"] = json!("Scope 3");
        assert!(title("Scope 3 emissions", &data).is_err());
        assert!(title("ISO 27001 compliance", &data).is_err());
        data["sources"]["sales"]["bucket_column"] = json!("date");
        data["sources"]["sales"]["rows"][0]["date"] = json!("Q3");
        assert!(title("Q3 revenue", &data).is_ok());
        for raw in ["12k", "x12000", "1e6%"] {
            data["sources"]["sales"]["rows"][0]["category"] = json!(raw);
            assert!(title(raw, &data).is_ok());
        }
    }
    #[test]
    fn unicode_case_matching_normalises_each_string_once() {
        let mut data = data();
        data["sources"]["sales"]["rows"][0]["name"] = json!("İSO 27001");
        data["sources"]["sales"]["rows"][0]["category"] = Value::Null;
        assert!(title("İSO 27001", &data).is_ok());
        assert!(title("ISO 27001", &data).is_err());
    }
    #[test]
    fn dates_are_parsed_as_whole_dates_and_ranges() {
        let mut data = data();
        assert!(title("As of 2026-01-01", &data).is_ok());
        assert!(title("As of 2026-01-02", &data).is_err());
        assert!(title("As of 2026-01-01T12:00:00Z", &data).is_err());
        assert!(title("Range 2026-01-01/2026-01-02", &data).is_err());
        data["sources"]["sales"]["rows"].as_array_mut().unwrap().push(json!({"date":"2026-01-02"}));
        assert!(title("Range 2026-01-01/2026-01-02", &data).is_ok());
        for raw in ["As of 2026-02-30", "As of 2026-1-1", "Value -01", "Year 2026", "As of x2026-01-01x"] { assert!(title(raw, &data).is_err(), "{raw}"); }
        data["sources"]["sales"]["bucket_column"] = Value::Null;
        assert!(title("As of 2026-01-02", &data).is_err());
        data["sources"]["sales"]["parameters"]["as_of"] = Value::Null;
        assert!(title("As of 2026-01-01", &data).is_err());
    }
    #[test]
    fn placeholders_resolve_one_typed_path_with_server_currency() {
        let data = data();
        for raw in ["Total {{sources.sales.rows[0].amount}}", "Total {{/sources/sales/totals/amount/sum}}"] {
            let rendered = title(raw, &data).unwrap();
            assert_eq!(rendered["title"], "Total 1588.00 AUD");
        }
        assert_eq!(title("Count {{/sources/sales/totals/amount/count}}", &data).unwrap()["title"], "Count 1");
        assert_eq!(title("As of {{sources.sales.rows[0].date}}", &data).unwrap()["title"], "As of 2026-01-01");
        for raw in ["{{sources.other.rows[0].amount}}", "{{/sources/sales/rows/0/category}}", "{{/sources/sales/parameters/limit}}", "{{/sources/sales/rows/0/other}}", "{{/sources/sales/rows/0/absent}}", "{{sources.sales}}", "{{/sources/sales/rows/0/amount~2}}", "{{/sources/sales/rows/0/amount}"] { assert!(title(raw, &data).is_err(), "{raw}"); }
        let error = title("{{sources.other.rows[0].amount}}", &data).unwrap_err();
        assert!(error.message.contains("{{/sources/sales/totals/amount/sum}}"));
        let mut missing = data.clone(); missing["sources"]["sales"]["rows"][0]["amount"] = Value::Null;
        assert!(title("{{sources.sales.rows[0].amount}}", &missing).is_err());
        let scenario = json!({"sources":{"s":{"view":"scenario","result":{"scenario":[{"value":1746.8,"delta":158.8,"delta_pct":10.0,"unit":"AUD"}]}}}});
        assert_eq!(title("After {{sources.s.result.scenario[0].value}}", &scenario).unwrap()["title"], "After 1746.80 AUD");
        assert_eq!(title("Change {{sources.s.result.scenario[0].delta_pct}}", &scenario).unwrap()["title"], "Change 10%");
        let nested = json!({"caption":"{{sources.sales.rows[0].amount}}", "columns":[{"label":"{{sources.sales.rows[0].quantity}}"}], "children":[{"title":"{{/sources/sales/rows/0/date}}"}]});
        let rendered = resolve(&nested, &data).unwrap();
        assert_eq!(rendered["caption"], "1588.00 AUD");
        assert_eq!(rendered["columns"][0]["label"], "12");
        assert_eq!(rendered["children"][0]["title"], "2026-01-01");
    }
    #[test]
    fn all_model_prose_fields_share_the_guard_and_substitution() {
        for field in ["title", "summary", "text", "label", "headline", "note", "reason", "caption", "legend", "why"] {
            let raw = json!({"nested":[{field:"Sales rose 12%"}]});
            assert!(resolve(&raw, &data()).unwrap_err().message.contains(field));
            let raw = json!({"nested":[{field:"Total {{sources.sales.rows[0].amount}}"}]});
            assert_eq!(resolve(&raw, &data()).unwrap()["nested"][0][field], "Total 1588.00 AUD");
        }
    }
    #[test]
    fn normalisation_precedes_token_matching() {
        let mut data = data(); data["sources"]["sales"]["rows"][0]["category"] = json!("Scope 12");
        for raw in ["Scope 1\u{200b}2", "Scope ١٢", "Scope １２", "Scope ¹²", "Scope ⑫", "Scope\u{2009}12"] { assert!(title(raw, &data).is_ok(), "{raw}"); }
        assert!(title("Scope ½", &data).is_err());
        assert_eq!(normalise("A\u{2060}B\u{2009}１２ ١٢ ½ ¹² ⑫"), "ab 12 12 1⁄2 12 12");
    }
}
