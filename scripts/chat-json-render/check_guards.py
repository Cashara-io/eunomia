"""Disable each guard in turn, require its regression test to fail, then restore.

Run from the repository root after the integration and library tests pass.
Logs and a JSON receipt go to the directory passed as the first argument.
Only one cargo process runs at a time. Sources are restored even on failure.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

root = Path(__file__).resolve().parents[2]
out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
env = {**os.environ, "CARGO_BUILD_JOBS": "2", "CARGO_PROFILE_DEV_DEBUG": "0",
       "CARGO_INCREMENTAL": "0", "RUST_MIN_STACK": "8388608"}
cases = [
    ("early filter limit", "backend/src/ontology/dataset.rs", ' || args.filters.len() > metric::MAX_FILTERS', '', "--test chat_ui dataset_filter_limit"),
    ("same-org nonmember", "backend/src/ontology/dataset.rs", 'let vault = super::resolve(db, owner, Some(&vault), Action::ReadMemories).await?;', 'let _ = (owner, Action::ReadMemories);', "--test chat_ui dataset_denies_same_org_nonmember"),
    ("restricted token", "backend/src/authz.rs", '    check_vault(vault)?;', '    // mutation: vault restriction disabled', "--test chat_ui dataset_denies_vault_restricted_token"),
    ("cross-vault link", "backend/src/store/ontology.rs", ' AND out.vault = $vault AND label IN $labels', ' AND label IN $labels', "--test chat_ui dataset_hides_cross_vault_links"),
    ("strict kind", "backend/src/ontology/dataset.rs", 'if super::strict(db, &vault).await? && !approved', 'if false && super::strict(db, &vault).await? && !approved', "--test chat_ui dataset_strict_kind_guard"),
    ("typed Stat scalar", "backend/src/views/ui.rs", 'if !trusted_number(data, path)', 'if false && !trusted_number(data, path)', "--lib views::ui::tests::numeric_bypass_matrix"),
    ("typed chart/table columns", "backend/src/views/ui.rs", 'if !numeric_column(source, key) || rows.iter().any(|r| !r[key].is_number() && !r[key].is_null())', 'if false && (!numeric_column(source, key) || rows.iter().any(|r| !r[key].is_number() && !r[key].is_null()))', "--lib views::ui::tests::numeric_bypass_matrix"),
    ("distribution shape", "backend/src/views/ui.rs", 'if parts.len() != 6 || parts[3] != "distributions" || parts[5] != shape || !numeric_column(source, parts[4]) || rows.iter().any(|r| fields.iter().any(|k| !r[k].is_number()))', 'if false && (parts.len() != 6 || parts[3] != "distributions" || parts[5] != shape || !numeric_column(source, parts[4]) || rows.iter().any(|r| fields.iter().any(|k| !r[k].is_number())))', "--lib views::ui::tests::numeric_bypass_matrix"),
    ("model-written digits", "backend/src/views/ui.rs", '&& value.as_str().is_some_and(|s| s.chars().any(char::is_numeric))', '&& false && value.as_str().is_some_and(|s| s.chars().any(char::is_numeric))', "--lib views::ui::tests::numeric_bypass_matrix"),
    ("mixed units", "backend/src/ontology/dataset.rs", 'if units.len() > 1', 'if false && units.len() > 1', "--test chat_ui dataset_cutoff_currency_and_mixed_units"),
    ("historical cutoff", "backend/src/ontology/dataset.rs", 'metric::parse_as_of(args.as_of.as_deref())?', 'metric::parse_as_of(None)?', "--test chat_ui dataset_cutoff_currency_and_mixed_units"),
    ("case-insensitive distinct", "backend/src/ontology/metric.rs", 'Value::String(s) => Value::String(s.to_lowercase()).to_string(),', 'Value::String(s) => Value::String(s.clone()).to_string(),', "--lib ontology::metric::tests::shared_aggregate_semantics_nulls_empty_and_case"),
    ("empty sum parity", "backend/src/ontology/metric.rs", 'Agg::Sum => Some(nums.iter().sum()),', 'Agg::Sum => if nums.is_empty() { None } else { Some(nums.iter().sum()) },', "--lib ontology::metric::tests::shared_aggregate_semantics_nulls_empty_and_case"),
    ("numeric aggregate datatype", "backend/src/ontology/dataset.rs", 'if matches!(a.op, Agg::Sum | Agg::Avg | Agg::Min | Agg::Max | Agg::Median) && !source.is_some_and', 'if false && matches!(a.op, Agg::Sum | Agg::Avg | Agg::Min | Agg::Max | Agg::Median) && !source.is_some_and', "--test chat_ui dataset_resolves_real_typed_values_and_mcp_catalog"),
]
results = []
if "--resume" in sys.argv and (out / "mutations.json").exists():
    results = [r for r in json.loads((out / "mutations.json").read_text()) if r["mutation_detected"]]
for name, filename, before, after, test in cases:
    if any(r["guard"] == name for r in results):
        continue
    subprocess.run(["df", "-h", "/"], check=True)
    if shutil.disk_usage(root).free < 8 * 1024**3:
        raise RuntimeError("Disk below 8 GiB. Stopping.")
    path = root / filename
    original = path.read_text()
    assert original.count(before) == 1, (name, original.count(before))
    try:
        path.write_text(original.replace(before, after))
        command = ["nice", "-n", "19", "cargo", "test", "--manifest-path", "backend/Cargo.toml", *test.split(), "--", "--nocapture"]
        result = subprocess.run(command, cwd=root, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        log = result.stdout.decode()
        (out / (name.replace(" ", "-").replace("/", "-") + ".log")).write_text(log)
        killed = result.returncode != 0 and "test result: FAILED" in log and "error[E" not in log
        results.append({"guard": name, "test": test, "mutation_detected": killed})
        (out / "mutations.json").write_text(json.dumps(results, indent=2) + "\n")
        print(name, "detected" if killed else "FAILED TO DETECT", flush=True)
        assert killed, (name, log[-3000:])
    finally:
        path.write_text(original)
