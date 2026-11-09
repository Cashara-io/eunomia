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
                // A binding cannot borrow even one connector from prose or another binding.
                overlaps.len() == 1 && overlaps[0].0 == *span && overlaps[0].1 == text[span.clone()]
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

/// Current-turn evidence for assistant text. Uses the same NFKC display, whole-span
/// scanner and placeholder resolver as UI prose. Previous assistant turns confer no trust.
#[derive(Default)]
pub(crate) struct ChatProse {
    numbers: HashSet<String>,
    sources: serde_json::Map<String, Value>,
}
fn numeric_key(span: &str) -> String {
    // Compare the whole value, never digit substrings or the parts of a range.
    // Formatting may add grouping commas, currency, or a percent sign.
    static CODE: OnceLock<Regex> = OnceLock::new();
    let code = CODE.get_or_init(|| Regex::new(&format!(r"^{CURRENCY_CODE} *| *{CURRENCY_CODE}$")).unwrap());
    let plain = code.replace_all(span, "");
    let plain = plain.trim_matches(|c: char| currency(c) || c.is_whitespace() || c == '%' || c == '‰').replace(',', "");
    let negative = plain.starts_with('-');
    let unsigned = plain.strip_prefix('-').or_else(|| plain.strip_prefix('+')).unwrap_or(&plain);
    let mut parts = unsigned.split('.');
    let whole = parts.next().unwrap_or("");
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some() || whole.is_empty() && fraction.is_empty()
        || !whole.bytes().chain(fraction.bytes()).all(|c| c.is_ascii_digit()) {
        return span.to_string();
    }
    let whole = whole.trim_start_matches('0');
    let fraction = fraction.trim_end_matches('0');
    let sign = if negative && (!whole.is_empty() || !fraction.is_empty()) { "-" } else { "" };
    format!("{sign}{}.{}", if whole.is_empty() { "0" } else { whole }, fraction)
}
impl ChatProse {
    pub(crate) fn text(&mut self, text: &str) {
        let text = normalise(text);
        for span in spans(&text) { self.numbers.insert(numeric_key(&text[span])); }
    }
    pub(crate) fn result(&mut self, value: &Value) {
        match value {
            Value::Number(n) => self.text(&n.to_string()),
            Value::String(s) => {
                self.text(s);
                if let Some(d) = date(&normalise(s)) { self.text(d.split('T').next().unwrap_or(&d)); }
            },
            Value::Array(a) => for v in a { self.result(v); },
            Value::Object(m) => for (key, v) in m {
                // Digits inside opaque record IDs are not numeric evidence.
                if key != "id" && key != "vault" && !key.ends_with("_id") && !key.ends_with("_ids") { self.result(v); }
            },
            _ => (),
        }
        if let Some(sources) = value.pointer("/data/sources").and_then(Value::as_object) { self.sources.extend(sources.clone()); }
    }
    pub(crate) fn check(&self, raw: &str) -> AppResult<String> {
        let data = serde_json::json!({"sources":self.sources});
        let (out, _) = interpolate(raw, &data, "assistant reply")?;
        let out = out.lines().map(display).collect::<Vec<_>>().join("\n");
        let normal = normalise(&out);
        for span in spans(&normal) {
            if !self.numbers.contains(&numeric_key(&normal[span.clone()])) {
                return Err(bad(format!("unsupported number in assistant reply: {}", &normal[span])));
            }
        }
        // Normalisation must not turn fullwidth braces into a new unresolved binding.
        if out.contains("{{") || out.contains("}}") { return Err(bad("unresolved placeholder in assistant reply")); }
        Ok(out)
    }
    pub(crate) fn safe(&self, raw: &str) -> String {
        self.check(raw).unwrap_or_else(|_| "I removed an unsupported number or unresolved placeholder from this reply. I could not provide a reliable written summary. Any completed tool results are shown above.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
