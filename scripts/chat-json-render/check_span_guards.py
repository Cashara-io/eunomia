#!/usr/bin/env python3
"""Remove each whole-span guard, require its regression assertion, restore bytes."""
import os
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
PROSE = ROOT / 'backend/src/views/ui/prose.rs'
LOGS = Path(os.environ.get('SPAN_GUARD_LOGS', '/private/tmp/eunomia-span-proof/mutations'))
LOGS.mkdir(parents=True, exist_ok=True)
MUTATIONS = [
    ('greedy-extraction', [('''[.,'_]''', '''[.'_]''')], 'whole_spans_refuse_gluing_across_every_source'),
    ('whole-binding-equality', [('overlaps.len() == 1 && overlaps[0].0 == *span && overlaps[0].1 == text[span.clone()]', 'true')], 'whole_spans_refuse_gluing_across_every_source'),
    ('whole-server-equality', [('server_ranges.get(span).is_some_and(|sources| sources.len() == 1)', 'server_ranges.iter().any(|(range, sources)| range.start <= span.start && span.start < range.end && sources.len() == 1)')], 'whole_spans_refuse_gluing_across_every_source'),
    ('unicode-minus', [(" { return '-'; }", ' { return c; }')], 'whole_spans_refuse_gluing_across_every_source'),
    ('display-whitespace', [("if c != ' ' || !out.ends_with(' ') { out.push(c); }", 'out.push(c);')], 'whole_spans_refuse_gluing_across_every_source'),
    ('normalisation', [('let stripped = format.replace_all(&compatible, "");', 'let stripped = format.replace_all(&compatible, "$0");')], 'whole_spans_refuse_gluing_across_every_source'),
]
for name, replacements, test in MUTATIONS:
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
    original = PROSE.read_bytes()
    try:
        source = original.decode()
        for old, new in replacements:
            assert old in source, (name, 'missing mutation anchor')
            source = source.replace(old, new)
        PROSE.write_text(source)
        env = dict(os.environ, CARGO_BUILD_JOBS='2', CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0', RUST_MIN_STACK='8388608', RUST_TEST_THREADS='1', TEST_HARDENED='1')
        log = LOGS / (name + '.log')
        with log.open('w') as output:
            code = subprocess.call(['nice', '-n', '19', 'cargo', 'test', '--lib', test], cwd=ROOT / 'backend', env=env, stdout=output, stderr=subprocess.STDOUT)
        output = log.read_text()
        assert code != 0 and test + ' ... FAILED' in output and 'could not compile' not in output, (name, output[-2500:])
        print(name + ': intended assertion failed', flush=True)
    finally:
        PROSE.write_bytes(original)
        assert PROSE.read_bytes() == original
