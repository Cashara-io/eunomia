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
