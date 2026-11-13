#!/usr/bin/env bash
# Rejection fixtures for the docs/data contract. No product runtime or Docker needed.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$ROOT" <<'PY'
import contextlib
import copy
import importlib.util
import io
import json
import shutil
import sys
import tempfile
from pathlib import Path

from jsonschema import ValidationError, SchemaError

root = Path(sys.argv[1])
spec = importlib.util.spec_from_file_location('catalogue_check', root / 'scripts/ci/check-compliance.py')
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)
original = {name: check.load(root / 'docs/compliance' / name) for name in
            ('controls.json', 'evidence-schema.json', 'asvs-checklist.json')}
with tempfile.TemporaryDirectory(prefix='eunomia-readiness-') as temporary:
    fixture = Path(temporary)
    shutil.copytree(root / 'docs/compliance', fixture / 'docs/compliance')
    for record in original['controls.json']['evidence_records']:
        for ref in record['references']:
            if 'path' in ref:
                target = fixture / ref['path']
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(root / ref['path'], target)

    def run(label, mutate=None, accepted=False):
        data = copy.deepcopy(original)
        if mutate:
            mutate(data)
        for name, value in data.items():
            (fixture / 'docs/compliance' / name).write_text(json.dumps(value))
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                check.check(fixture)
        except (ValueError, KeyError, TypeError, OSError, ValidationError, SchemaError):
            if accepted:
                raise
        else:
            if not accepted:
                raise AssertionError(f'accepted invalid fixture: {label}')
        print(f'pass: {label}')

    def catalogue(fn):
        return lambda data: fn(data['controls.json'])

    run('valid catalogue', accepted=True)
    run('duplicate capability', catalogue(lambda c: c['controls'].append(c['controls'][0])))
    run('dangling standard', catalogue(lambda c: c['controls'][0]['mappings'][0].update(standard='absent')))
    run('dangling clause', catalogue(lambda c: c['controls'][0]['mappings'][0].update(clauses=['0.0.0'])))
    run('dangling evidence', catalogue(lambda c: c['controls'][0].update(evidence=['EV-C01-99'])))
    run('duplicate evidence', catalogue(lambda c: c['evidence_records'].append(c['evidence_records'][0])))
    run('missing source', catalogue(lambda c: c['standards'][0].update(sources=[])))
    run('dangling source', catalogue(lambda c: c['standards'][0].update(sources=['absent'])))
    run('missing version', catalogue(lambda c: c['standards'][0].pop('version')))
    run('unexplained scope exclusion', catalogue(lambda c: c['exclusions'][0].update(reason='')))
    run('unreviewed normative mapping', catalogue(lambda c: c['standards'][1].update(mapping_review='reviewed')))
    run('source inspection cannot pass operation', catalogue(lambda c: c['evidence_records'][0].update(operation='passes measured check')))
    run('evidence id must match capability', catalogue(lambda c: c['evidence_records'][0].update(id='EV-C02-1')))
    run('newest evidence must state the control status', catalogue(lambda c: c['controls'][0].update(status='Missing' if c['controls'][0]['status'] != 'Missing' else 'Done')))
    run('invalid example', lambda d: d['evidence-schema.json']['examples'][0].update(implementation='verified'))
    run('unexplained example exclusion', lambda d: d['evidence-schema.json']['examples'][0].update(operation='not applicable'))
    run('invalid schema', lambda d: d['evidence-schema.json']['properties']['owner'].update(type='nonexistent'))
    run('incomplete ASVS checklist', lambda d: d['asvs-checklist.json']['requirements'].pop())
    run('substituted ASVS requirement', lambda d: d['asvs-checklist.json']['requirements'][0].update(id='99.99.99'))
    run('unexplained ASVS exclusion', lambda d: d['asvs-checklist.json']['requirements'][0].update(applicability='not applicable', applicability_reason=''))
    run('invented ASVS verification', lambda d: d['asvs-checklist.json']['requirements'][0].update(verification='pass'))
    try:
        duplicate = fixture / 'duplicate.json'
        duplicate.write_text('{"id":"C01","id":"C02"}')
        check.load(duplicate)
    except ValueError:
        print('pass: duplicate JSON keys')
    else:
        raise AssertionError('duplicate JSON keys accepted')
print('readiness rejection fixtures: 21 passed')
PY
