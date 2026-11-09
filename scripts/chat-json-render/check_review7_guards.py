#!/usr/bin/env python3
"""Disable each provenance fix, require its assertion failure, restore exact bytes."""
import os
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
LOGS = Path(os.environ.get('REVIEW7_LOGS', '/private/tmp/eunomia-pr203-r7-fix/mutations'))
LOGS.mkdir(parents=True, exist_ok=True)
PROSE = 'backend/src/views/ui/prose.rs'
UI = 'backend/src/views/ui.rs'
MUTATIONS = [
    ('normalised-display', PROSE, 'Ok(display(&out))', 'Ok(out)', 'displayed_prose_removes_bidi_and_format_controls', '--lib'),
    ('spaced-prefix', PROSE, r'(?:{modifier} *|', r'(?:{modifier}|', 'spaced_modifiers_cannot_change_bound_counts', '--lib'),
    ('spaced-suffix', PROSE, r'(?: *{modifier}|', r'(?:{modifier}|', 'spaced_modifiers_cannot_change_bound_counts', '--lib'),
    ('source-alias', UI, 'prose::identifier(&name)?;', 'let _ = &name;', 'display_normalisation_modifiers_and_source_aliases_are_guarded', '--test'),
    ('displayed-selectors', UI, 'if !known {', 'if false && !known {', 'identifiers_cannot_invent_displayed_numeric_spans', '--lib'),
    ('element-identifiers', UI, 'prose::identifier(id)?;', 'let _ = id;', 'identifiers_cannot_invent_displayed_numeric_spans', '--lib'),
    ('source-label', UI, 'out.push_str(&format!("\\nSource {}:\\n", index + 1));', 'out.push_str(&format!("\\nSource {}:\\n", sources.keys().nth(index).unwrap()));', 'identifiers_cannot_invent_displayed_numeric_spans', '--lib'),
]
for name, filename, old, new, test, target in MUTATIONS:
    deadline = time.monotonic() + 7200
    while True:
        subprocess.run(['df', '-h', '/'], check=True)
        other = subprocess.run(['pgrep', '-x', 'cargo'], capture_output=True, text=True)
        print('pgrep -x cargo: ' + (other.stdout.strip() or 'none'), flush=True)
        if other.returncode == 1 and shutil.disk_usage('/').free >= 7 * 1024**3:
            break
        if time.monotonic() >= deadline:
            raise SystemExit('Cargo/disk gate blocked for two hours')
        time.sleep(180)
    path = ROOT / filename
    original = path.read_bytes()
    try:
        source = original.decode()
        assert source.count(old) == 1, (name, 'mutation anchor must be unique')
        path.write_text(source.replace(old, new, 1))
        env = dict(os.environ, CARGO_BUILD_JOBS='2', CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0', RUST_MIN_STACK='8388608', RUST_TEST_THREADS='1', TEST_HARDENED='1')
        command = ['nice', '-n', '19', 'cargo', 'test', target]
        if target == '--test':
            command.append('chat_ui')
        command.append(test)
        log = LOGS / (name + '.log')
        with log.open('w') as output:
            code = subprocess.call(command, cwd=ROOT / 'backend', env=env, stdout=output, stderr=subprocess.STDOUT)
        output = log.read_text()
        assert code != 0 and test + ' ... FAILED' in output and 'could not compile' not in output, (name, output[-2500:])
        print(name + ': intended assertion failed', flush=True)
    finally:
        path.write_bytes(original)
        assert path.read_bytes() == original
