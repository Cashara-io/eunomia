#!/usr/bin/env python3
"""Disable each bound-prose guard, require an assertion failure, restore exact bytes."""
import os
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
LOGS = Path(os.environ.get('PROSE_GUARD_LOGS', '/private/tmp/chat-json-render-v5/mutations'))
LOGS.mkdir(parents=True, exist_ok=True)
FILE = ROOT / 'backend/src/views/ui/prose.rs'
MUTATIONS = [
 ('placeholder-type', 'if trusted_number(data, &path) && let Some(n)', 'if true && let Some(n)', 'placeholders_resolve_one_typed_path_with_server_currency'),
 ('placeholder-path', 'let path = pointer(binding)?;', 'let path = if binding.contains("other") { "/sources/sales/rows/0/amount".into() } else { pointer(binding)? };', 'placeholders_resolve_one_typed_path_with_server_currency'),
 ('raw-digit-rejection', 'if field(key) && let Some(s) = v.as_str()', 'if false && field(key) && let Some(s) = v.as_str()', 'unrelated_values_and_identifier_bypasses_are_rejected'),
 ('verbatim-scope', '["kind", "entities", "relations", "start", "affected", "result"]', '["kind", "entities", "relations", "start", "affected", "result", "headline", "text"]', 'verbatim_tokens_are_scoped_to_resolved_server_strings'),
 ('verbatim-boundaries', "if text[..start].chars().next_back().is_some_and(token) || text[end..].chars().next().is_some_and(token)", "if false && (text[..start].chars().next_back().is_some_and(token) || text[end..].chars().next().is_some_and(token))", 'verbatim_tokens_are_scoped_to_resolved_server_strings'),
 ('date-parsing', 'if date(m.as_str()).is_some_and(|d| self.dates.contains(&d))', 'if true', 'dates_are_parsed_as_whole_dates_and_ranges'),
 ('normalisation', 'let stripped = format.replace_all(text, "");', 'let stripped = format.replace_all(text, "$0");', 'normalisation_precedes_token_matching'),
]
original = FILE.read_bytes()
try:
 for name, old, new, test in MUTATIONS:
  deadline = time.monotonic() + 7200
  while True:
   subprocess.run(['df', '-h', '/'], check=True)
   cargo = subprocess.run(['pgrep', '-x', 'cargo'], capture_output=True, text=True)
   print('pgrep -x cargo: '+(cargo.stdout.strip() or 'none'), flush=True)
   if cargo.returncode and shutil.disk_usage('/').free >= 7*1024**3: break
   if time.monotonic() >= deadline: raise SystemExit('Cargo/disk gate blocked for two hours')
   time.sleep(180)
  source = original.decode()
  assert old in source, (name, 'missing mutation anchor')
  FILE.write_text(source.replace(old, new, 1))
  env = dict(os.environ, CARGO_BUILD_JOBS='2', CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0', RUST_MIN_STACK='8388608', RUST_TEST_THREADS='1')
  with (LOGS / (name+'.log')).open('w') as log:
   code = subprocess.call(['nice', '-n', '19', 'cargo', 'test', '--lib', test], cwd=ROOT/'backend', env=env, stdout=log, stderr=subprocess.STDOUT)
  output = (LOGS / (name+'.log')).read_text()
  assert code != 0 and test+' ... FAILED' in output and 'could not compile' not in output, (name, output[-1800:])
  FILE.write_bytes(original)
  print(name+': intended regression failed, restored', flush=True)
finally:
 FILE.write_bytes(original)
