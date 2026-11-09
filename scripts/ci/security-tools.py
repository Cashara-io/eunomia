#!/usr/bin/env python3
"""Install reviewed release archives, checking checked-in SHA-256 pins."""
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import sys
import tarfile
import urllib.request

pins = json.loads(Path(__file__).with_name('security').joinpath('tools.json').read_text())
destination = Path(os.environ.get('SECURITY_BIN', str(Path.home() / '.local/bin')))
destination.mkdir(parents=True, exist_ok=True)
arch = {'aarch64': 'arm64', 'x86_64': 'amd64'}.get(platform.machine(), platform.machine())
system = platform.system().lower()
for repo, pin in pins.items():
    name = repo.split('/')[-1]
    if sys.argv[1:] and name not in sys.argv[1:]:
        continue
    suffix = f'{system}_{arch}.tar.gz'
    if name == 'gitleaks' and arch == 'amd64':
        suffix = f'{system}_x64.tar.gz'
    if name == 'cargo-deny':
        suffix = {('linux', 'amd64'): 'x86_64-unknown-linux-musl.tar.gz',
                  ('linux', 'arm64'): 'aarch64-unknown-linux-musl.tar.gz',
                  ('darwin', 'arm64'): 'aarch64-apple-darwin.tar.gz'}[(system, arch)]
    archive, expected = next((k, v) for k, v in pin['archives'].items() if k.endswith(suffix))
    url = f'https://github.com/{repo}/releases/download/{pin["version"]}/{archive}'
    data = urllib.request.urlopen(url).read()
    if hashlib.sha256(data).hexdigest() != expected:
        raise SystemExit(f'Checksum mismatch: {name}')
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as bundle:
        member = next(m for m in bundle.getmembers() if Path(m.name).name == name and m.isfile())
        target = destination / name
        target.write_bytes(bundle.extractfile(member).read())
        target.chmod(0o755)
    print(f'Installed {name} {pin["version"]}')
