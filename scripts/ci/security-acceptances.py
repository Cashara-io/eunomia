#!/usr/bin/env python3
"""Small shared policy for scoped, expiring findings. Import without a package."""
from datetime import date, datetime, timezone
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load(path=None, today=None):
    today = today or datetime.now(timezone.utc).date()
    entries = json.loads(Path(path or ROOT / 'docs/compliance/security-acceptances.json').read_text())
    seen = set()
    for entry in entries:
        for field in ('scanner', 'id', 'package', 'scope', 'reason', 'owner', 'expires'):
            if not isinstance(entry.get(field), str) or not entry[field].strip():
                raise ValueError(f'Acceptance needs {field}')
        key = tuple(entry[k] for k in ('scanner', 'id', 'package', 'scope'))
        if key in seen:
            raise ValueError(f'Duplicate acceptance: {key}')
        seen.add(key)
        if date.fromisoformat(entry['expires']) <= today:
            raise ValueError(f'Expired acceptance: {entry["id"]} ({entry["package"]})')
    return entries


def gate(findings, entries):
    failed = False
    for finding in findings:
        match = next((e for e in entries if all(e[k] == finding[k] for k in
                     ('scanner', 'id', 'package', 'scope'))), None)
        if match:
            print(f'ACCEPTED {finding["id"]} {finding["package"]}: {match["reason"]} '
                  f'Owner: {match["owner"]}; expires: {match["expires"]}')
        else:
            print(f'REJECTED {finding["id"]} {finding["package"]} ({finding["scope"]})')
            failed = True
    return 1 if failed else 0


if __name__ == '__main__':
    load()
    print('All acceptances have owners and future expiry dates')
