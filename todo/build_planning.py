"""Regenerate planning CSV/JSON exports from planning/plan_source.json (stdlib)."""
from pathlib import Path
import csv
import json

P = Path(__file__).resolve().parent
GATES = {'M0': '', 'M1': 'M0', 'M2': 'M1', 'M3': 'M2', 'M4': 'M3',
         'M5': 'M4', 'M6': 'M3;M4', 'M7': 'M5;M6', 'M8': 'M7'}

def build():
    source = json.loads((P / 'planning/plan_source.json').read_text(encoding='utf-8'))
    refs = {s['id']: s for s in json.loads((P / 'sources.json').read_text(encoding='utf-8'))}
    data = {name: source[name] for name in ('implementation_backlog', 'acceptance_tests', 'feature_inventory')}
    cases = data['acceptance_tests']
    case_ids = {t['id'] for t in cases}
    for name, rows in data.items():
        assert len({r['id'] for r in rows}) == len(rows), name + ': duplicate ID'
        for row in rows:
            for section in row['spec_sections'].split(','):
                assert 1 <= int(section) <= 27, (row['id'], section)
            ref_field = 'source_ids' if name == 'implementation_backlog' else 'reference_ids'
            if ref_field in row:
                ids = [s for s in row[ref_field].split(',') if s]
                assert set(ids) <= refs.keys(), row['id']
                row['source_urls'] = ' ; '.join(refs[s]['url'] for s in ids)
            if name == 'implementation_backlog':
                m = row['milestone']
                row['dependency_gates'] = GATES[m]
                row['priority'] = 'P2' if m == 'M8' else 'P0' if m in ('M0','M1','M2','M3','M7') else 'P1'
                exact = set(filter(None, row.get('acceptance_test_ids', '').split(';')))
                assert exact <= case_ids, row['id']
                sections = set(row['spec_sections'].split(',')) - {'2','22','24','26'}
                row['test_family_candidates'] = ';'.join(t['id'] for t in cases
                    if sections.intersection(t['spec_sections'].split(','))
                    and ((m == 'M8') == (t['family'] == 'Advanced') or m == 'M7'))
        rows.sort(key=(lambda r: (r['milestone'], r['id'])) if name == 'implementation_backlog' else lambda r: r['id'])
        (P / 'planning' / f'{name}.json').write_text(json.dumps(rows, indent=2) + '\n', encoding='utf-8')
        with (P / 'planning' / f'{name}.csv').open('w', newline='', encoding='utf-8') as f:
            writer = csv.DictWriter(f, fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)
    print('; '.join(f'{len(rows)} {name}' for name, rows in data.items()))

if __name__ == '__main__':
    build()
