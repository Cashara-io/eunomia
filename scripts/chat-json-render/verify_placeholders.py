#!/usr/bin/env python3
"""Audit successful live UI calls for model placeholders and server substitution.

Run after verify_numbers.py, which independently recomputes the bound data from CSV.
"""
from decimal import Decimal, ROUND_HALF_UP
import json
from pathlib import Path
import re
import sys

pattern = re.compile(r'\{\{([^{}]+)\}\}')
fields = {'title', 'summary', 'text', 'label', 'headline', 'note', 'reason', 'caption', 'legend', 'why'}
def prose(value, path=()):
    if isinstance(value, dict):
        for k, v in value.items():
            if k in fields and isinstance(v, str): yield path+(k,), v
            else: yield from prose(v, path+(k,))
    elif isinstance(value, list):
        for i, v in enumerate(value): yield from prose(v, path+(i,))
def at(value, path):
    for k in path: value = value[int(k)] if isinstance(value, list) else value[k]
    return value

total = 0
receipts = sorted(Path(sys.argv[1]).glob('answer-*.json'))
for file in receipts:
    receipt = json.loads(file.read_text())
    pending = None
    success = 0
    final_count = 0
    for message in receipt['history']:
        for call in message.get('tool_calls') or []:
            if call['function']['name'] == 'show_view': pending = json.loads(call['function']['arguments'])
        if message['role'] != 'tool' or pending is None: continue
        result = json.loads(message['content'])
        if result.get('view') != 'ui': continue
        count = 0
        for path, raw in prose(pending['spec']):
            matches = list(pattern.finditer(raw))
            if not matches: continue
            rendered = at(result['spec'], path)
            assert '{{' not in rendered, (file.name, path, rendered)
            assert rendered in result['text'], (file.name, path, 'missing from terminal summary')
            cursor = 0
            expected = ''
            substitutions = []
            for m in matches:
                binding = m[1]
                parts = binding.strip('/').split('/') if binding.startswith('/') else re.sub(r'\[([0-9]+)\]', r'.\1', binding).split('.')
                parts = [p.replace('~1', '/').replace('~0', '~') for p in parts]
                value = at(result['data'], parts)
                assert isinstance(value, (int, float, str)) and not isinstance(value, bool), (file.name, binding, value)
                source = result['data']['sources'][parts[1]]
                rest = parts[2:]
                column = next((c for c in source.get('columns', []) if len(rest) > 2 and c['key'] == (rest[2] if rest[0] == 'rows' else rest[1])), {})
                is_count = rest == ['total_rows'] or rest[-1] in ('count', 'distinct', 'n', 'n_entities', 'n_changed')
                unit = None if is_count else column.get('unit') or source.get('metric', {}).get('unit')
                if not is_count and rest[0] == 'result' and rest[1] in ('baseline', 'scenario'):
                    unit = '%' if rest[-1] == 'delta_pct' else source['result'][rest[1]][int(rest[2])].get('unit')
                if isinstance(value, str): shown = value
                else:
                    places = 1 if unit in ('%', 'percent', 'percentage', 'pct') else 2
                    shown = format(Decimal(str(value)).quantize(Decimal(10) ** -places, rounding=ROUND_HALF_UP), 'f')
                    if not (unit and re.fullmatch('[A-Z]{3}', unit)): shown = shown.rstrip('0').rstrip('.') if '.' in shown else shown
                    if unit: shown += '%' if unit in ('%', 'percent', 'percentage', 'pct') else ' '+unit
                substitutions.append(shown)
                expected += raw[cursor:m.start()] + shown
                cursor = m.end()
            expected += raw[cursor:]
            assert rendered == expected, (file.name, raw, rendered, substitutions)
            count += len(matches)
        final_count = count
        success += 1
        total += count
        pending = None
    assert success and final_count, (file.name, 'final successful UI has no model text placeholders')
    print(f'{file.name}: model used placeholders; server substituted them in spec and terminal text')
print(f'{total} placeholders substituted across {len(receipts)} live answers. Numeric data checked separately by verify_numbers.py.')
