#!/usr/bin/env python3
"""Check the compatibility oracle against the pinned upstream source, without building main."""
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MAIN = "6116a00ff86fed992a917863e9a3515f70373b84"
source = subprocess.check_output(["git", "show", f"{MAIN}:backend/src/ontology/metric.rs"], cwd=ROOT, text=True)
start = source.index("fn compute(")
body = source.index("{", start)
depth = 0
for end in range(body, len(source)):
    depth += (source[end] == "{") - (source[end] == "}")
    if depth == 0:
        break
expected = source[start:end + 1].replace("fn compute(", "fn compute_main(", 1).replace(
    "        Agg::Avg | Agg::Ratio => None,",
    '        Agg::Avg | Agg::Ratio => None,\n        Agg::Median => panic!("median did not exist on main"),',
)
actual = (ROOT / "backend/src/ontology/metric_main_fixture.rs").read_text().split("fn compute_main", 1)[1]
assert "".join(expected.split()) == "".join(("fn compute_main" + actual).split())
print(f"Oracle matches main {MAIN}, except its name and the unreachable Median arm.")
