#!/usr/bin/env python3
"""Exercise real scanners, plus the acceptance gate's failure paths."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
from datetime import date

spec = importlib.util.spec_from_file_location('scanner', Path(__file__).with_name('security-scan.py'))
scanner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scanner)
with tempfile.TemporaryDirectory() as temporary:
    root = Path(temporary)
    findings = scanner.scan('npm', 'scripts/ci/security/fixtures/vulnerable', root / 'audit.json')
    assert any(f['package'] == 'lodash' and f['id'].startswith('GHSA-') for f in findings), 'Vulnerable fixture not detected'
    assert scanner.policy.gate(findings, []) == 1
    findings = scanner.scan('gitleaks', 'scripts/ci/security/fixtures/secret', root / 'secret.json')
    assert any(f['id'] == 'github-pat' for f in findings), 'Fake GitHub PAT not detected'
    # The exact fixture bytes must not escape in a report or normal gate output.
    secret = Path('scripts/ci/security/fixtures/secret/fake.txt').read_text().strip().split('=', 1)[1]
    assert secret not in (root / 'secret.json').read_text(), 'Secret leaked in report'
    result = subprocess.run(['python3', 'scripts/ci/security-scan.py', 'gitleaks',
                             'scripts/ci/security/fixtures/secret', str(root / 'secret.json')], capture_output=True, text=True)
    assert result.returncode == 1
    assert secret not in result.stdout + result.stderr, 'Secret leaked to log'
    entry = dict(scanner='test', id='test-id', package='test-package', scope='test',
                 owner='Test owner', reason='Test reason', expires='2026-11-10')
    path = root / 'acceptances.json'
    path.write_text(json.dumps([entry]))
    assert scanner.policy.load(path, date(2026, 11, 9)) == [entry]
    try:
        scanner.policy.load(path, date(2026, 11, 10))
    except ValueError:
        pass
    else:
        raise AssertionError('Acceptance valid on its expiry date')
    assert scanner.policy.gate([entry], [entry]) == 0
    assert scanner.policy.gate([dict(entry, scope='elsewhere')], [entry]) == 1
    del entry['owner']
    path.write_text(json.dumps([entry]))
    try:
        scanner.policy.load(path, date(2026, 11, 9))
    except ValueError:
        pass
    else:
        raise AssertionError('Acceptance without owner allowed')
print('Seeded vulnerable dependency rejected; fake secret caught and redacted; acceptance failures enforced')

# Architecture inventories must retain different versions of the same package.
spec = importlib.util.spec_from_file_location('sbom', Path(__file__).with_name('security-sbom.py'))
sbom = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sbom)
with tempfile.TemporaryDirectory() as temporary:
    paths = []
    for arch, version in [('amd64', '1'), ('arm64', '2')]:
        path = Path(temporary) / (arch + '.json')
        path.write_text(json.dumps({'bomFormat': 'CycloneDX', 'components': [
            {'name': 'runtime', 'type': 'library', 'version': version, 'bom-ref': 'runtime'}]}))
        paths.append(path)
    merged = sbom.combine(paths, 'manifest')
    assert {c['version'] for c in merged['components']} == {'1', '2'}
    assert len({c['bom-ref'] for c in merged['components']}) == 2
print('Multi-architecture SBOM retains both inventories without colliding references')

# Replacing an accepted credential at the same path and line must be rejected.
with tempfile.TemporaryDirectory() as temporary:
    root = Path(temporary)
    source = root / 'changing-secret'
    source.mkdir()
    fixture = source / 'fake.txt'
    fixture.write_text(Path('scripts/ci/security/fixtures/secret/fake.txt').read_text())
    initial = scanner.scan('gitleaks', str(source), root / 'initial.json')
    assert initial
    accepted = [dict(f, owner='Fixture owner', reason='Original fixture', expires='2099-01-01') for f in initial]
    fixture.write_text(fixture.read_text().rstrip() + 'extra-content\n')
    changed = scanner.scan('gitleaks', str(source), root / 'changed.json')
    assert changed
    assert scanner.policy.gate(changed, accepted) == 1
print('Changed source cannot reuse a secret acceptance at the same file and line')

# cargo-deny combines check failures into a bitmask, not just exit code 1.
original_run = scanner.run
with tempfile.TemporaryDirectory() as temporary:
    output = Path(temporary) / 'cargo.jsonl'
    diagnostic = {'type': 'diagnostic', 'fields': {
        'severity': 'error', 'code': 'vulnerability',
        'advisory': {'id': 'RUSTSEC-TEST'},
        'graphs': [{'Krate': {'name': 'vulnerable', 'version': '1.0.0'}}]}}
    report = json.dumps(diagnostic) + '\n' + json.dumps({'type': 'summary', 'fields': {'advisories': {'errors': 1}}})
    scanner.run = lambda *args, **kwargs: subprocess.CompletedProcess(args, 5, '', report)
    try:
        findings = scanner.scan('cargo', 'backend', output)
        assert scanner.policy.gate(findings, []) == 1
        accepted = [dict(f, owner='Fixture owner', reason='Temporary fixture acceptance', expires='2099-01-01') for f in findings]
        assert scanner.policy.gate(findings, accepted) == 0
        scanner.run = lambda *args, **kwargs: subprocess.CompletedProcess(args, 5, '', json.dumps(diagnostic))
        try:
            scanner.scan('cargo', 'backend', output)
        except RuntimeError:
            pass
        else:
            raise AssertionError('Incomplete scanner report accepted')
    finally:
        scanner.run = original_run
print('Cargo bitmask errors are gated and incomplete reports fail closed')
