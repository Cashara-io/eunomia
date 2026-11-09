"""Recompute every numeric dataset and scenario field in live chat receipts from the CSV.

Run with python3 -I scripts/chat-json-render/verify_numbers.py CSV ANSWER_DIRECTORY.
The receipts are the real chat history, not fixture specs or model summaries.
"""
import csv
import datetime
import decimal
import json
import math
import pathlib
import statistics
import sys

D = decimal.Decimal
records = list(csv.DictReader(open(sys.argv[1], newline="")))
for row in records:
    for key in ("amount", "quantity", "share"):
        row[key] = D(row[key]) if row[key] else None
checks = 0


def close(actual, expected, path):
    global checks
    if actual is None or expected is None:
        assert actual == expected, (path, actual, expected)
    else:
        assert abs(D(str(actual)) - D(str(expected))) <= D("0.01"), (path, actual, expected)
    checks += 1


def numeric_tree(actual, expected, path):
    if isinstance(expected, dict):
        for key, value in expected.items():
            numeric_tree(actual[key], value, f"{path}/{key}")
    elif isinstance(expected, list):
        assert len(actual) == len(expected), (path, len(actual), len(expected))
        for i, value in enumerate(expected):
            numeric_tree(actual[i], value, f"{path}/{i}")
    elif expected is None or isinstance(expected, (int, float, D)) and not isinstance(expected, bool):
        close(actual, expected, path)


def bucket(value, grain):
    date = datetime.date.fromisoformat(value[:10])
    if grain == "day":
        return date.isoformat()
    if grain == "week":
        return date.strftime("%G-W%V")
    if grain == "month":
        return date.strftime("%Y-%m")
    if grain == "quarter":
        return f"{date.year}-Q{(date.month - 1)//3 + 1}"
    return str(date.year)


def dataset(query, actual):
    selected = records
    for f in query.get("filters", []):
        def pick(row):
            a, b = row[f["property"]], f["value"]
            if isinstance(a, D):
                b = D(str(b))
            return {"eq": lambda: a == b, "ne": lambda: a != b,
                    "gt": lambda: a is not None and a > b, "gte": lambda: a is not None and a >= b,
                    "lt": lambda: a is not None and a < b, "lte": lambda: a is not None and a <= b,
                    "in": lambda: a in b}[f["op"]]()
        selected = list(filter(pick, selected))
    group = query.get("group_by")
    aggregates = query.get("aggregates", [])
    if aggregates:
        groups = {}
        for row in selected:
            key = bucket(row[group], query["bucket"]) if query.get("bucket") else row.get(group)
            groups.setdefault(key, []).append(row)
        if not group and not groups:
            groups[None] = []
        expected = []
        for key, members in groups.items():
            out = {group: key} if group else {}
            for agg in aggregates:
                values = [r[agg["property"]] for r in members if r.get(agg.get("property")) is not None]
                op = agg["op"]
                if op == "count":
                    value = len(values) if agg.get("property") else len(members)
                elif op in ("distinct", "count_distinct"):
                    value = len(set(v.lower() if isinstance(v, str) else v for v in values))
                elif not values:
                    value = 0 if op == "sum" else None
                else:
                    value = {"sum": lambda: sum(values), "avg": lambda: statistics.mean(values),
                             "min": lambda: min(values), "max": lambda: max(values),
                             "median": lambda: statistics.median(values)}[op]()
                out[agg["name"]] = value
            expected.append(out)
    else:
        expected = [{"name": r["name"], **{p: r[p] for p in query["properties"]}} for r in selected]
    key = group if group else "name"
    if not aggregates or group:
        expected_by_key = {str(r.get(key)): r for r in expected}
        expected = [expected_by_key[str(r.get(key))] for r in actual["rows"]]
    numeric_tree(actual["rows"], expected, "rows")
    close(actual["total_rows"], len(expected), "total_rows")
    for column in actual["columns"]:
        name = column["key"]
        nums = [r[name] for r in expected if isinstance(r.get(name), (D, int, float))]
        values = [r[name] for r in expected if r.get(name) is not None]
        numeric_tree(actual["totals"][name], {"sum": sum(nums),
                     "avg": statistics.mean(nums) if nums else None, "count": len(values),
                     "min": min(nums) if nums else None, "max": max(nums) if nums else None,
                     "median": statistics.median(nums) if nums else None,
                     "distinct": len(set(v.lower() if isinstance(v, str) else v for v in values))}, f"totals/{name}")
        if name not in actual.get("distributions", {}):
            continue
        nums = sorted(float(n) for n in nums)
        if not nums:
            continue
        n = min(10, max(1, math.ceil(math.sqrt(len(nums)))))
        low, high = nums[0], nums[-1]
        step = (high - low or 1)/n
        counts = [0]*n
        for value in nums:
            counts[min(n-1, math.floor((value-low)/step))] += 1
        quartiles = statistics.quantiles(nums, n=4, method="inclusive") if len(nums)>1 else [nums[0]]*3
        expected_dist = {"bins": [{"start": low+i*step, "end": low+(i+1)*step, "count": count} for i,count in enumerate(counts)],
                         "box": [{"min":low,"q1":quartiles[0],"median":statistics.median(nums),"q3":quartiles[2],"max":high}],
                         "density": [{"value":low+(i+0.5)*step,"count":count} for i,count in enumerate(counts)]}
        numeric_tree(actual["distributions"][name], expected_dist, f"distributions/{name}")


def scenario(query, actual):
    result = actual["result"]
    before = sum(r["amount"] for r in records)
    factor = D(str(query["scenario"]["changes"][0]["scale"]["factor"]))
    after = before*factor
    close(result["baseline"][0]["value"], before, "baseline")
    close(result["scenario"][0]["value"], after, "scenario")
    close(result["scenario"][0]["delta"], after-before, "delta")
    close(result["scenario"][0]["delta_pct"], (factor-1)*100, "delta_pct")
    close(result["n_changed"], len(records), "n_changed")
    for side in ["baseline", "scenario"]:
        close(result[side][0]["n_entities"], len(records), f"{side}/n_entities")
    by_name = {r["name"]:r for r in records}
    for row in result["changed"]:
        close(row["before"], by_name[row["name"]]["amount"], "changed/before")
        close(row["after"], by_name[row["name"]]["amount"]*factor, "changed/after")


for file in sorted(pathlib.Path(sys.argv[2]).glob("answer-*.json")):
    receipt = json.loads(file.read_text())
    calls = {}
    pending = []
    views = 0
    for message in receipt["history"]:
        for call in message.get("tool_calls") or []:
            calls[call["id"]] = call["function"]
            pending.append(call["function"])
        if message["role"] != "tool":
            continue
        call = pending.pop(0)
        shown = json.loads(message["content"])
        if shown.get("view") != "ui":
            continue
        args = json.loads(call["arguments"])
        for name, query in args["sources"].items():
            source = shown["data"]["sources"][name]
            if query["type"] == "dataset":
                dataset(query, source)
            elif query["type"] == "scenario":
                scenario(query, source)
            else:
                raise AssertionError(f"Unaudited source: {query['type']}")
        views += 1
    assert views, f"No successful UI in {file}"
    print(f"{file.name}: matched, {views} UI")
print(f"Verified {checks} numeric fields within 0.01.")
