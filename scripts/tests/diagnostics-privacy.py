#!/usr/bin/env python3
"""Exercise the actual collector, then inspect a stored ZIP containing its output."""
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location('diagnostics', Path(__file__).resolve().parents[1] / 'diagnostics.py')
diag = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diag)

SECRETS = ['314159265358979323846', '8675309', '1234', '1234567890123456789012345678901234567890', '61412345678', '4111111111111111']

class Privacy(unittest.TestCase):
    def test_numeric_canaries_never_enter_zip(self):
        records = []
        for value in SECRETS:
            records += ['provider error: ' + json.dumps({'count': int(value)}),
                        json.dumps({'fields': {'count': int(value), 'duration_ms': int(value)}, 'level': 'WARN'}),
                        json.dumps({'name': 'diagnostics.worker_started', 'target': 'eunomia_backend::jobs::worker', 'fields': {'concurrency': int(value)}, 'message': value})]
        # Small numbers that happen to fit a bound still require the known event AND field.
        records += [json.dumps({'name': 'unknown', 'fields': {'count': 1234}}),
                    json.dumps({'name': 'diagnostics.worker_started', 'target': 'eunomia_backend::jobs::worker', 'fields': {'count': 1234, 'concurrency': 4}, 'message': SECRETS[0]})]
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            (root / '.env').write_text('NUMERIC_SECRET=8675309\nPIN=1234\n')
            (root / 'diagnostics-requested').write_text('a' * 32)
            with patch.object(diag, 'command', side_effect=lambda args: '\n'.join(records) if args[0] == 'logs' else ''):
                diag.collect(root)
            buf = io.BytesIO()
            with zipfile.ZipFile(buf, 'w', compression=zipfile.ZIP_STORED) as bundle:
                for file in (root / 'diagnostics').iterdir():
                    bundle.writestr(file.name, file.read_bytes())
            raw = buf.getvalue().decode('utf-8', errors='replace')
            for secret in SECRETS:
                self.assertNotIn(secret, raw)
            self.assertIn('job worker started', raw)
            self.assertIn('"concurrency": 4', raw)

    def test_only_named_typed_bounded_fields(self):
        for value in [True, False, -1, 1.5, 100001, '3', None, float('inf')]:
            row = json.loads(diag.safe_logs(json.dumps({'name': 'diagnostics.worker_stopped', 'target': 'eunomia_backend::jobs::worker', 'fields': {'released': value}})))
            self.assertEqual(row['fields'], {})
        self.assertEqual(json.loads(diag.safe_logs('prefix {"level":"WARN","count":3}'))['level'], 'unknown')
        self.assertEqual(json.loads(diag.safe_logs('{"name": [], "level": []}'))['message'], '<redacted>')

if __name__ == '__main__':
    unittest.main()
