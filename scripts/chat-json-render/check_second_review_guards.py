#!/usr/bin/env python3
"""Remove one guard at a time, require its regression failure, then restore the source."""
import argparse
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument("suite", choices=["frontend", "backend"])
parser.add_argument("--logs", type=Path, default=Path("/tmp/pr192-guards"))
args = parser.parse_args()
args.logs.mkdir(parents=True, exist_ok=True)
UI = "frontend/src/components/chat/ui/"
BACK = "backend/src/"
# Each expected name comes from the test runner's failed-test list.
frontend = [
    ("numeric-access", UI + "data.ts", 'return typeof value === "number" && Number.isFinite(value) ? value : null;', 'return typeof value === "number" && Number.isFinite(value) ? value : 0;', "UI numeric access never defaults"),
    ("required-y", UI + "Charts.tsx", '[...new Set([y, ...(p.series || [])])', '[...(p.series?.length ? p.series : [y])', "every chart checks y"),
    ("meter-minimum", UI + "Blocks.tsx", 'min = number(value.min);', 'min = number(value.min) ?? 0;', "meter requires its minimum"),
    ("zero-recurrence", UI + "data.ts", 'const n = number(value);', 'const n = number(value) ?? 0;', "UI numeric access never defaults"),
    ("table-metadata", UI + "Blocks.tsx", 'format: props.formats?.[c.key] ?? inferred?.find(source => source.key === c.key)?.format ?? c.format,', 'format: c.format,', "explicit table columns inherit server formats"),
    ("unitless-count", UI + "UiView.tsx", 'segments[3] === "total_rows" ||', 'false ||', "server types override model formats"),
    ("server-format", UI + "UiView.tsx", 'e.props.format = count ? "number" : column ? formats[column.key] : "number";', '// metadata override removed', "server types override model formats"),
    ("missing-stat-group", UI + "Blocks.tsx", '["number", "money", "percent"].includes(c.format ?? "") || number(row[c.key]) != null', 'number(row[c.key]) != null', "stat groups retain a missing typed measurement"),
    ("drawing-range", UI + "Charts.tsx", 'if (rows.some(r => Object.values(r).some(v => typeof v === "number" && !drawable(v))))', 'if (false && rows.some(r => Object.values(r).some(v => typeof v === "number" && !drawable(v))))', "unsafe finite chart values"),
]
backend = [
    ("words-grammar", BACK + "views/ui.rs", '                validate_words(text)?;', '                let _ = text;', "model_words_grammar_covers_all_prose"),
    ("aggregate-alias", BACK + "ontology/dataset.rs", '#[serde(deny_unknown_fields)]\npub struct Aggregate', 'pub struct Aggregate', "aggregate_names_are_server_generated"),
    ("datetime-precision", BACK + "ontology/metric.rs", 'distinct.push(Value::String(format!("{:?}|{:?}|{:?}|{:?}", s.num, s.text.as_deref().map(str::to_lowercase), s.boolean, s.date)));', 'distinct.push(to_json(props[p].datatype, s).unwrap_or(Value::Null));', "main_metric_fixture_preserves_existing_results"),
    ("sum-order", BACK + "ontology/metric.rs", '    let n = nums.len();', '    nums.sort_by(f64::total_cmp);\n    let n = nums.len();', "main_metric_fixture_preserves_existing_results"),
]

def run(command, cwd, log):
    if shutil.disk_usage("/").free < 6 * 1024**3:
        raise RuntimeError("STOP: disk below 6 GiB")
    env = dict(os.environ, CARGO_BUILD_JOBS="2", CARGO_PROFILE_DEV_DEBUG="0", CARGO_INCREMENTAL="0", RUST_MIN_STACK="8388608")
    with log.open("w") as out:
        process = subprocess.Popen(command, cwd=cwd, env=env, stdout=out, stderr=subprocess.STDOUT, start_new_session=True)
        while process.poll() is None:
            if shutil.disk_usage("/").free < 6 * 1024**3:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait()
                raise RuntimeError("STOP: disk below 6 GiB")
            time.sleep(1)
    return process.returncode, log.read_text()

for name, filename, old, new, expected in frontend if args.suite == "frontend" else backend:
    path = ROOT / filename
    original = path.read_text()
    assert old in original, (name, "mutation anchor missing")
    try:
        path.write_text(original.replace(old, new, 1))
        command = ["bun", "test", "scripts/chat-ui-values.test.jsx"] if args.suite == "frontend" else ["nice", "-n", "19", "cargo", "test", "--lib", expected]
        code, output = run(command, ROOT / args.suite, args.logs / f"{name}.log")
        assert code != 0 and expected in output and ("(fail)" in output if args.suite == "frontend" else f"{expected} ... FAILED" in output), (name, "expected regression did not fail", code)
        assert "could not compile" not in output, (name, "compile failure is not regression evidence")
        print(f"{name}: expected regression failed", flush=True)
    finally:
        path.write_text(original)
print("All mutated source files restored.")
