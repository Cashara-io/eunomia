#!/usr/bin/env python3
"""Run scanners. Scanner errors and unknown report formats fail closed."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

spec = importlib.util.spec_from_file_location('policy', Path(__file__).with_name('security-acceptances.py'))
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)
ROOT = policy.ROOT


def run(command, cwd=None, statuses=(0,)):
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
    if result.returncode not in statuses:
        # Never print scanner output here: it may contain a discovered credential.
        raise RuntimeError(f'{command[0]} failed with exit {result.returncode}')
    return result


def scan(kind, scope, output):
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    findings = []
    if kind in ('bun', 'npm'):
        command = ['bun', 'audit', '--json'] if kind == 'bun' else ['npm', 'audit', '--json', '--package-lock-only']
        result = run(command, cwd=scope, statuses=(0, 1))
        report = json.loads(result.stdout)
        output.write_text(json.dumps(report, indent=2) + '\n')
        if kind == 'bun':
            if not isinstance(report, dict):
                raise ValueError('Unexpected Bun audit report')
            for package, advisories in report.items():
                for advisory in advisories:
                    findings.append(dict(scanner='audit', id=advisory['url'].split('/')[-1], package=package, scope=scope))
        else:
            if 'vulnerabilities' not in report or 'error' in report:
                raise ValueError('Unexpected npm audit report')
            for package, item in report['vulnerabilities'].items():
                for advisory in item['via']:
                    if isinstance(advisory, dict):
                        findings.append(dict(scanner='audit', id=advisory['url'].split('/')[-1], package=advisory['name'], scope=scope))
        if result.returncode and not findings:
            raise RuntimeError('Audit failed without recognised findings')
    elif kind == 'cargo':
        result = run(['cargo-deny', '--manifest-path', 'backend/Cargo.toml', '--locked',
                      '--format', 'json', 'check', 'advisories', 'licenses', 'bans', 'sources'], statuses=tuple(range(16)))
        output.write_text(result.stdout + result.stderr)
        summary = None
        for line in output.read_text().splitlines():
            try:
                item = json.loads(line)
            except json.JSONDecodeError:
                continue
            if item.get('type') == 'summary':
                summary = item['fields']
            if item.get('type') != 'diagnostic' or item['fields']['severity'] != 'error':
                continue
            fields = item['fields']
            advisory = fields.get('advisory')
            identifier = advisory['id'] if advisory else 'cargo-' + fields['code']
            for graph in fields.get('graphs', []):
                crate = graph['Krate']
                findings.append(dict(scanner='cargo-deny', id=identifier,
                                     package=crate['name'] + '@' + crate['version'], scope='backend'))
            if not fields.get('graphs'):
                raise RuntimeError('Unrecognised cargo-deny diagnostic')
        if summary is None or (result.returncode and not findings):
            raise RuntimeError('cargo-deny failed without a complete report')
    elif kind == 'gitleaks':
        # Scan tracked working-tree files, including dotfiles. Exclude only the seeded fixture directory.
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary)
            files = run(['git', 'ls-files', '-z']).stdout.split('\0') if scope == 'repo' else None
            if files is not None:
                for file in files:
                    if not file or file.startswith('scripts/ci/security/fixtures/'):
                        continue
                    source = ROOT / file
                    if source.is_symlink() or not source.is_file():
                        continue
                    destination = target / file
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_bytes(source.read_bytes())
            else:
                target = Path(scope).resolve()
            result = run(['gitleaks', 'dir', str(target), '--redact=100', '--no-banner',
                          '--report-format', 'json', '--report-path', str(output.resolve())], statuses=(0, 1))
            report = json.loads(output.read_text())
            for item in report:
                file = str(Path(item['File']).relative_to(target)) if Path(item['File']).is_absolute() else item['File']
                findings.append(dict(scanner='gitleaks', id=item['RuleID'], package=f'{file}:{item["StartLine"]}', scope=scope))
            if result.returncode and not findings:
                raise RuntimeError('Gitleaks failed without findings')
    elif kind == 'grype':
        result = run(['grype', 'sbom:' + scope, '-o', 'json'])
        report = json.loads(result.stdout)
        output.write_text(json.dumps(report, indent=2) + '\n')
        for match in report['matches']:
            if match['vulnerability']['severity'] not in ('High', 'Critical'):
                continue
            findings.append(dict(scanner='grype', id=match['vulnerability']['id'],
                                 package=match['artifact']['name'] + '@' + match['artifact']['version'], scope=os.environ['IMAGE_SCOPE']))
    else:
        raise ValueError(f'Unknown scanner: {kind}')
    return findings


if __name__ == '__main__':
    entries = policy.load()
    findings = scan(*sys.argv[1:])
    sys.exit(policy.gate(findings, entries))
