#!/usr/bin/env python3
"""Generate an image inventory, retaining a labelled backend lockfile inventory."""
import copy
import json
import re
from pathlib import Path
import subprocess
import sys


def combine(paths, name):
    result = {'bomFormat': 'CycloneDX', 'specVersion': '1.6', 'version': 1,
              'metadata': {'component': {'type': 'container', 'name': name}}, 'components': []}
    for number, path in enumerate(paths):
        source = json.loads(Path(path).read_text())
        assert source['bomFormat'] == 'CycloneDX'
        for component in source.get('components', []):
            component = copy.deepcopy(component)
            # Ref IDs can overlap between architecture and lockfile inventories.
            component['bom-ref'] = f'inventory-{number}:' + component.get('bom-ref', component['name'])
            component.setdefault('properties', []).append({'name': 'eunomia:inventory-source', 'value': str(path)})
            result['components'].append(component)
    if not result['components']:
        raise ValueError('Empty SBOM')
    return result


if __name__ == '__main__':
    mode, output, name, *sources = sys.argv[1:]
    destination = Path(output)
    destination.parent.mkdir(parents=True, exist_ok=True)
    if mode == 'image':
        subprocess.run(['syft', 'scan', sources[0], '-o', f'cyclonedx-json={output}'], check=True)
        if name == 'backup':
            inventory = json.loads(destination.read_text())
            version = re.search(r'FROM surrealdb/surrealdb:v([^ ]+) AS', Path('backend/scripts/backup/Dockerfile').read_text())[1]
            binary = next((c for c in inventory['components'] if c['name'] == '/usr/local/bin/surreal'), {})
            inventory['components'].append({
                'type': 'application', 'name': 'surrealdb', 'version': version,
                'bom-ref': 'eunomia:surrealdb-backup-tool',
                'purl': 'pkg:generic/surrealdb@' + version,
                'hashes': binary.get('hashes', []),
                'properties': [{'name': 'eunomia:version-source', 'value': 'backend/scripts/backup/Dockerfile FROM tag'}]})
            destination.write_text(json.dumps(inventory, indent=2) + '\n')
        if name == 'backend':
            lock_inventory = destination.with_suffix('.lock.cdx.json')
            subprocess.run(['syft', 'scan', 'dir:backend', '--override-default-catalogers', 'rust-cargo-lock-cataloger',
                            '-o', f'cyclonedx-json={lock_inventory}'], check=True)
            inventory = combine([destination, lock_inventory], name)
            destination.write_text(json.dumps(inventory, indent=2) + '\n')
    elif mode == 'merge':
        destination.write_text(json.dumps(combine(sources, name), indent=2) + '\n')
    else:
        raise ValueError(mode)
