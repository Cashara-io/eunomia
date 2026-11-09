#!/usr/bin/env python3
"""Mutate each PR 203 guard, require assertion failures, and restore exact bytes."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
LOGS = Path(os.environ.get('REVIEW4_LOGS', '/private/tmp/chat-json-render-v6/mutations'))
LOGS.mkdir(parents=True, exist_ok=True)
PROSE = 'backend/src/views/ui/prose.rs'
MUTATIONS = [
    ('literal-rendering', 'frontend/src/components/chat/ui/Blocks.tsx', [
        ('import Link from "next/link";', 'import Link from "next/link";\nimport ReactMarkdown from "react-markdown";'),
        ('return <div className="whitespace-pre-wrap">{text}</div>;', 'return <ReactMarkdown>{text}</ReactMarkdown>;'),
    ], 'UI prose never reparses Markdown entities or escapes'),
    ('final-substitution-boundaries', PROSE, [('overlaps.len() == 1 && overlaps[0].0 == *span && overlaps[0].1 == text[span.clone()]', 'true')], 'final_placeholder_boundaries_refuse_numeric_synthesis'),
    ('letter-bearing-phrase', PROSE, [('let mut out = Vec::new();\n    for span', 'let mut out = vec![server.clone()];\n    for span')], 'bare_numeric_server_strings_never_authorise_digits'),
    ('turn-cutoff-propagation', 'backend/src/views/mod.rs', [('TURN_STARTED.try_with(|start| *start).unwrap_or_default()', 'surrealdb::types::Datetime::default()')], 'turn_snapshot_refuses_new_names_and_text_values'),
    ('pre-turn-snapshot', 'backend/src/store/ontology.rs', [('AND created_at < $before AND (updated_at = NONE OR updated_at < $before) AND', 'AND')], 'turn_snapshot_refuses_new_names_and_text_values'),
]
for name, filename, replacements, test in MUTATIONS:
    if len(sys.argv) > 1 and sys.argv[1] != name:
        continue
    frontend = filename.startswith('frontend/')
    if not frontend:
        deadline = time.monotonic() + 7200
        while True:
            subprocess.run(['df', '-h', '/'], check=True)
            other = subprocess.run(['pgrep', '-x', 'cargo'], capture_output=True, text=True)
            print('pgrep -x cargo: ' + (other.stdout.strip() or 'none'), flush=True)
            if other.returncode and shutil.disk_usage('/').free >= 7 * 1024**3:
                break
            if time.monotonic() >= deadline:
                raise SystemExit('Cargo/disk gate blocked for two hours')
            time.sleep(180)
    path = ROOT / filename
    original = path.read_bytes()
    try:
        mutated = original.decode()
        for old, new in replacements:
            assert old in mutated, (name, 'missing mutation anchor')
            mutated = mutated.replace(old, new)
        path.write_text(mutated)
        env = dict(os.environ, CARGO_BUILD_JOBS='2', CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0', RUST_MIN_STACK='8388608', RUST_TEST_THREADS='1', TEST_HARDENED='1')
        if frontend:
            command = ['bun', 'test', 'scripts/chat-ui-values.test.jsx']
        else:
            target = ['--test', 'chat_ui'] if test == 'turn_snapshot_refuses_new_names_and_text_values' else ['--lib']
            command = ['nice', '-n', '19', 'cargo', 'test', *target, test]
        log = LOGS / (name + '.log')
        with log.open('w') as output:
            code = subprocess.call(command, cwd=ROOT / ('frontend' if frontend else 'backend'), env=env, stdout=output, stderr=subprocess.STDOUT)
        output = log.read_text()
        expected = '(fail) ' + test if frontend else test + ' ... FAILED'
        assert code != 0 and expected in output and 'could not compile' not in output, (name, output[-2500:])
        print(name + ': intended assertion failed, restored', flush=True)
    finally:
        path.write_bytes(original)
