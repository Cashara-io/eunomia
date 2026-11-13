#!/usr/bin/env python3
"""Check readiness data, cross-references and evidence examples without network access."""
import json
import hashlib
import re
import sys
from collections import Counter
from pathlib import Path

from jsonschema import Draft202012Validator, FormatChecker, ValidationError, SchemaError


def require(condition, message):
    if not condition:
        raise ValueError(message)


def load(path):
    def unique_keys(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f'duplicate JSON key: {key}')
            result[key] = value
        return result
    return json.loads(path.read_text(), object_pairs_hook=unique_keys)


def indexed(rows, label):
    ids = [row['id'] for row in rows]
    require(len(ids) == len(set(ids)), f'duplicate {label} ids')
    return {row['id']: row for row in rows}


def nonempty(value):
    return isinstance(value, str) and bool(value.strip())


def check(root):
    directory = root / 'docs/compliance'
    catalogue = load(directory / 'controls.json')
    schema = load(directory / 'evidence-schema.json')
    checklist = load(directory / 'asvs-checklist.json')
    Draft202012Validator.check_schema(schema)
    validator = Draft202012Validator(schema, format_checker=FormatChecker())
    for example in schema['examples']:
        validator.validate(example)
    require(catalogue['catalogue_version'] == '1.0.0', 'missing/unsupported catalogue version')
    require(catalogue['evidence_schema_version'] == schema['properties']['schema_version']['const'], 'schema version mismatch')
    require(re.fullmatch(r'[0-9a-f]{40}', catalogue['snapshot']['commit']), 'missing snapshot version')
    sources = indexed(catalogue['sources'], 'source')
    for source in sources.values():
        require(nonempty(source['title']) and nonempty(source['limits']), 'source needs title and limits')
        require(source['urls'] and all(url.startswith('https://') for url in source['urls']), 'missing source URL')
        require(nonempty(source['reviewed_on']), 'missing source review date')
    standards = indexed(catalogue['standards'], 'standard')
    require(set(standards) == {'ASVS', 'SOC', 'I27', 'APP', 'GDPR', 'E8', 'CIS', 'I42', 'AI'}, 'expected nine standards')
    for standard in standards.values():
        require(nonempty(standard['version']), f"missing version: {standard['id']}")
        require(standard['sources'] and set(standard['sources']) <= sources.keys(), 'dangling/missing source')
        require(standard['clauses'] and len(standard['clauses']) == len(set(standard['clauses'])), 'duplicate/missing clauses')
        require(nonempty(standard['reviewed_on']), 'missing standard review date')
        if standard['id'] in {'SOC', 'I27', 'I42', 'CIS'}:
            require(standard['mapping_review'] == 'provisional', 'authorised-text review still pending')
    exclusions = indexed(catalogue['exclusions'], 'exclusion')
    require('model-data-path' in exclusions and 'hosted-service-attestation' in exclusions, 'scope exclusions missing')
    for exclusion in exclusions.values():
        require(nonempty(exclusion['reason']) and exclusion['owner'] in {'Eunomia', 'customer', 'shared'}, 'unexplained exclusion')
    controls = indexed(catalogue['controls'], 'capability')
    require(set(controls) == {f'C{n:02}' for n in range(1, 37)}, 'expected C01 to C36')
    records = indexed(catalogue['evidence_records'], 'evidence')
    used_evidence = set()
    mapped_standards = set()
    for control in controls.values():
        require(control['status'] in {'Done', 'Partial', 'Missing'}, 'invalid implementation status')
        require(control['owner'] in {'Eunomia', 'customer', 'shared'}, 'invalid control owner')
        require(control['track'] == ['code', 'documentation'], 'product and policy tracks must stay separate')
        require(nonempty(control['finding']) and nonempty(control['acceptance_check']), 'missing finding or acceptance check')
        require(control['evidence'] and set(control['evidence']) <= records.keys(), 'dangling/missing evidence')
        for eid in control['evidence']:
            require(records[eid]['capability_id'] == control['id'], 'cross-capability evidence')
        # earlier captures stay as history (README: history preserved); the newest one states the current status
        latest = max(control['evidence'], key=lambda eid: int(eid.rsplit('-', 1)[1]))
        require(records[latest]['implementation'] == control['status'], 'evidence status mismatch')
        used_evidence.update(control['evidence'])
        require(control['mappings'], 'missing mappings')
        require(len(control['mappings']) == len({m['standard'] for m in control['mappings']}), 'duplicate standard mapping')
        for mapping in control['mappings']:
            require(mapping['standard'] in standards, 'dangling standard mapping')
            standard = standards[mapping['standard']]
            require(mapping['clauses'] and set(mapping['clauses']) <= set(standard['clauses']), 'dangling clause mapping')
            require(len(mapping['clauses']) == len(set(mapping['clauses'])), 'duplicate clause mapping')
            require(mapping['review'] == standard['mapping_review'], 'mapping review mismatch')
            require(nonempty(mapping['applicability']), 'unexplained mapping applicability')
            mapped_standards.add(mapping['standard'])
    require(mapped_standards == standards.keys(), 'unmapped standard')
    require(used_evidence == records.keys(), 'orphaned evidence')
    for record in records.values():
        validator.validate(record)
        require(record['id'].startswith('EV-' + record['capability_id'] + '-'), 'evidence id/capability mismatch')
        require(record['subject']['release_sha'] == catalogue['snapshot']['commit'], 'source snapshot mismatch')
        for ref in record['references']:
            require(ref['line_end'] >= ref['line_start'], 'invalid evidence line range')
            if 'path' in ref:
                path = root / ref['path']
                require(path.resolve().is_relative_to(root.resolve()) and path.is_file(), 'missing/unsafe evidence path')
                require(ref['line_end'] <= len(path.read_text().splitlines()), 'evidence line outside file')
    require(checklist['version'] and checklist['standard_version'] == '5.0.0', 'missing ASVS version')
    require(checklist['source'] in sources and checklist['upstream_url'] in sources['S1']['urls'], 'missing ASVS source')
    require(re.fullmatch(r'[0-9a-f]{64}', checklist['upstream_sha256']), 'missing ASVS source digest')
    requirements = indexed(checklist['requirements'], 'ASVS requirement')
    require(Counter(r['level'] for r in requirements.values()) == {1: 70, 2: 183}, 'incomplete ASVS L1/L2 checklist')
    level3 = checklist['level3_reference_ids']
    require(len(level3) == 92 and len(set(level3)) == 92, 'incomplete ASVS L3 reference index')
    inventory = [f"{r['id']}:{r['level']}" for r in requirements.values()] + [f'{rid}:3' for rid in level3]
    # Digest of the official v5.0.0 ID/level pairs, independent of our crosswalk.
    require(hashlib.sha256('\n'.join(sorted(inventory)).encode()).hexdigest() ==
            'b7765d77d469d61b5e6eae0216add3732d6c561cf5f1489e4f982b40f905b38b', 'ASVS ID/level inventory drift')
    asvs_clauses = set(standards['ASVS']['clauses'])
    require(asvs_clauses <= requirements.keys() | set(level3), 'unknown ASVS requirement mapping')
    for item in requirements.values():
        require(re.fullmatch(r'\d+\.\d+\.\d+', item['id']), 'invalid ASVS id')
        require(item['applicability'] in {'unknown', 'applicable', 'not applicable'}, 'invalid ASVS applicability')
        require(nonempty(item['applicability_reason']), 'unexplained ASVS applicability/exclusion')
        if item['applicability'] == 'not applicable':
            require(nonempty(item.get('approved_by')), 'ASVS exclusion needs an approver')
        require(item['verification'] == 'unknown' and item['evidence'] == [], 'PR01 does not verify ASVS requirements')
        require(nonempty(item['chapter']) and nonempty(item['section']), 'missing ASVS headings')
    # L3 contributions may occur in the capability crosswalk, outside the L2 checklist.
    require(all(re.fullmatch(r'\d+\.\d+\.\d+', c) for c in asvs_clauses), 'ASVS mappings need requirement ids')
    for path in directory.rglob('*'):
        if path.is_file():
            require('\u2014' not in path.read_text(), f'em dash in {path}')
    provisional = sum(len(m['clauses']) for c in controls.values() for m in c['mappings'] if m['review'] == 'provisional')
    print(f"readiness catalogue: {len(controls)} capabilities, {dict(Counter(c['status'] for c in controls.values()))}, {len(standards)} standards, {provisional} provisional clause links; ASVS 70 L1 + 183 L2 unknown")


if __name__ == '__main__':
    try:
        check(Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[2])
    except (ValueError, KeyError, TypeError, OSError, ValidationError, SchemaError) as error:
        sys.exit(f'check-compliance: {error}')
