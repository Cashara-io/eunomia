//! Resolve model prose once, at the server boundary. Values are never authorised by equality.
use std::collections::HashSet;
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
fn normalise(text: &str) -> String {
    static FORMAT: OnceLock<Regex> = OnceLock::new();
    static DIGIT: OnceLock<Regex> = OnceLock::new();
    let format = FORMAT.get_or_init(|| Regex::new(r"\p{Cf}").unwrap());
    let digit = DIGIT.get_or_init(|| Regex::new(r"^\p{Nd}$").unwrap());
    let stripped = format.replace_all(text, "");
    let mapped: String = stripped.nfkc().map(|c| {
        if c.is_whitespace() { return ' '; }
        if c.is_ascii() || !digit.is_match(&c.to_string()) { return c; }
        let mut start = c as u32;
        while start > 0 && char::from_u32(start - 1).is_some_and(|p| digit.is_match(&p.to_string())) { start -= 1; }
        char::from_digit((c as u32 - start) % 10, 10).unwrap()
    }).collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}
fn token(c: char) -> bool { currency(c) || c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.' | '%' | '+' | '\u{2044}') }
fn following_token(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some('.') => chars.next().is_some_and(token),
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
    fn check(&self, raw: &str, path: &str) -> AppResult<()> {
        let text = normalise(raw);
        let mut covered = vec![false; text.len()];
        for server in &self.strings {
            for allowed in phrases(server) {
                for (start, _) in text.match_indices(&allowed) {
                    let end = start + allowed.len();
                    if text[..start].chars().next_back().is_some_and(token) || following_token(&text[end..]) { continue; }
                    covered[start..end].fill(true);
                }
            }
        }
        static ISO: OnceLock<Regex> = OnceLock::new();
        let iso = ISO.get_or_init(|| Regex::new(r"\b[0-9]{4}-[0-9]{2}-[0-9]{2}(?:[tT][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:[zZ]|[+-][0-9]{2}:[0-9]{2}))?\b").unwrap());
        for m in iso.find_iter(&text) {
            // ISO ranges may use '/', '..' or a dash. Each endpoint must be authorised.
            if date(m.as_str()).is_some_and(|d| self.dates.contains(&d)) {
                covered[m.start()..m.end()].fill(true);
            } else {
                return Err(bad(format!("unsupported raw ISO date in {path}; use a matching applied date or bucket label")));
            }
        }
        if text.char_indices().any(|(i, c)| c.is_numeric() && !covered[i]) {
            return Err(bad(format!("unsupported raw digits in {path}: {raw:?}; replace each number with a bound placeholder such as {{{{/sources/sales/totals/amount/sum}}}}. Raw digits require a verbatim phrase with an adjacent letter-bearing word in a server string predating the turn, or a matching applied ISO date or bucket label. Number words are unchecked.")));
        }
        Ok(())
    }
}
// A digit must travel with an adjacent letter-bearing word from one server string.
// Never join words from different records, or authorise a bare numeric token.
fn phrases(server: &str) -> Vec<String> {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let word = WORD.get_or_init(|| Regex::new(r"[\p{L}\p{N}_/%.+\x{2044}-]+").unwrap());
    let server = normalise(server);
    let words: Vec<_> = word.find_iter(&server).collect();
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if !w.as_str().chars().any(char::is_numeric) { continue; }
        if w.as_str().chars().any(char::is_alphabetic) { out.push(w.as_str().into()); }
        for j in [i.checked_sub(1), (i + 1 < words.len()).then_some(i + 1)].into_iter().flatten() {
            let neighbour = words[j];
            let start = w.start().min(neighbour.start());
            let end = w.end().max(neighbour.end());
            let gap = if j < i { &server[neighbour.end()..w.start()] } else { &server[w.end()..neighbour.start()] };
            if neighbour.as_str().chars().any(char::is_alphabetic) && gap.chars().all(char::is_whitespace) {
                out.push(server[start..end].into());
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
        let count = matches!(rest.as_slice(), ["total_rows"] | ["totals", _, "count" | "distinct"] | ["metric", "n_entities"] | ["metric", "series", _, "n"] | ["result", "n_changed"] | ["result", "baseline" | "scenario", _, "n_entities"] | ["distributions", _, "bins" | "density", _, "count"]);
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
fn render(raw: &str, data: &Value, allowed: &Allowed, field: &str) -> AppResult<String> {
    let mut out = String::new();
    let mut rest = raw;
    let mut substitutions = Vec::new();
    while let Some(start) = rest.find("{{") {
        allowed.check(&rest[..start], field)?;
        out.push_str(&rest[..start]);
        let end = rest[start+2..].find("}}").ok_or_else(|| bad(format!("unclosed text placeholder in {field}; use {{{{/sources/name/rows/0/amount}}}}")))? + start + 2;
        let value = bound(&rest[start+2..end], data)?;
        let offset = out.len();
        out.push_str(&value);
        substitutions.push((offset, out.len()));
        rest = &rest[end+2..];
    }
    if rest.contains("}}") { return Err(bad(format!("unexpected placeholder closing braces in {field}"))); }
    allowed.check(rest, field)?;
    out.push_str(rest);
    for (start, end) in substitutions {
        let left = normalise_edge(&out[..start], false);
        let right = normalise_edge(&out[end..], true);
        if left.chars().next_back().is_some_and(numeric_neighbour) || right.chars().next().is_some_and(numeric_neighbour) {
            return Err(bad(format!("text placeholder in {field} touches a numeric token; separate each placeholder from digits, numeric symbols and e/E")));
        }
    }
    Ok(out)
}
fn normalise_edge(text: &str, first: bool) -> String {
    let normalised = normalise(&format!("a{text}a"));
    let inside = &normalised[1..normalised.len() - 1];
    (if first { inside.chars().next() } else { inside.chars().next_back() }).map(|c| c.to_string()).unwrap_or_default()
}
fn currency(c: char) -> bool {
    static SYMBOL: OnceLock<Regex> = OnceLock::new();
    SYMBOL.get_or_init(|| Regex::new(r"^\p{Sc}$").unwrap()).is_match(&c.to_string())
}
fn numeric_neighbour(c: char) -> bool {
    c.is_numeric() || currency(c) || matches!(c, '.' | ',' | '%' | '+' | '-' | '/' | 'e' | 'E' | '\u{2044}')
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
    fn final_placeholder_boundaries_refuse_numeric_synthesis() {
        let p = "{{/sources/sales/total_rows}}";
        for raw in [format!("Orders {p}{p}"), format!("{p}{{{{/sources/sales/rows/0/date}}}}"), format!("{p}%"), format!("${p}"), format!("{p}."), format!("{p}e{p}"), format!("e{p}"), format!("{p}E"), format!("{p}１"), format!("{p}\u{200b}{p}"), format!("{p}\u{200b}%"), format!("€{p}"), format!("{p}\u{200b}E")] {
            assert!(title(&raw, &data()).is_err(), "{raw}");
        }
        for raw in [format!("Orders ({p}), then {p} records"), format!("{p} records"), format!("Count {p}"), format!("{p}\u{200b} records")] {
            assert!(title(&raw, &data()).is_ok(), "{raw}");
        }
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
