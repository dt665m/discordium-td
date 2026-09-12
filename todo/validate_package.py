"""Validate the engineering handoff and rerun its narrow contract suite.

Python 3.11+; standard library only. HTML regeneration uses the separately pinned
mistune dependency. This script does not run production acceptance workloads.
"""
from __future__ import annotations

import csv
import hashlib
from html.parser import HTMLParser
import json
from math import ceil
from pathlib import Path
import platform
import re
import subprocess
import sys
import tomllib
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parent
GATES = {'M0': '', 'M1': 'M0', 'M2': 'M1', 'M3': 'M2', 'M4': 'M3',
         'M5': 'M4', 'M6': 'M3;M4', 'M7': 'M5;M6', 'M8': 'M7'}
EXPECTED_COUNTS = {'implementation_backlog': 103, 'acceptance_tests': 156,
                   'feature_inventory': 93}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def load_json(relative: str):
    return json.loads((ROOT / relative).read_text(encoding='utf-8'))


class Links(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.ids: set[str] = set()
        self.links: list[str] = []
        self.duplicates: set[str] = set()

    def handle_starttag(self, tag: str, attrs) -> None:
        attrs = dict(attrs)
        identity = attrs.get('id')
        if identity:
            if identity in self.ids:
                self.duplicates.add(identity)
            self.ids.add(identity)
        if tag == 'a' and attrs.get('href'):
            self.links.append(attrs['href'])


def source_links_local(text: str) -> str:
    return re.sub(r'\[S(\d{2})\]\((?:ENGINEERING_SPEC\.(?:md|html))?#s\d{2}\)',
                  lambda m: f'[S{m[1]}](#s{m[1]})', text)


def embedded_source(relative: str, section: int, title: str) -> str:
    lines = (ROOT / relative).read_text(encoding='utf-8').splitlines()
    lines[0] = f'## {section}. {title}'
    for i in range(1, len(lines)):
        lines[i] = re.sub(r'^## (\d+)\. ', lambda m: f'### {section}.{m[1]}. ', lines[i])
    return source_links_local('\n'.join(lines)).strip()


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate() -> dict:
    checks: list[str] = []
    source = load_json('planning/plan_source.json')
    refs = load_json('sources.json')
    ref_ids = {r['id'] for r in refs}
    require(len(refs) == len(ref_ids) == 62, 'Source inventory must contain 62 unique references')
    require(ref_ids == {f'S{i:02d}' for i in range(1, 63)}, 'Source IDs not contiguous/preserved')
    require(source['revision'] == 2, 'Canonical planning revision is not 2')
    planning = {}
    for name, expected in EXPECTED_COUNTS.items():
        rows = load_json(f'planning/{name}.json')
        planning[name] = rows
        require(len(rows) == expected, f'{name}: unexpected count')
        require(len({r['id'] for r in rows}) == expected, f'{name}: duplicate IDs')
        with (ROOT / 'planning' / f'{name}.csv').open(newline='', encoding='utf-8') as f:
            csv_rows = list(csv.DictReader(f))
        require(csv_rows == rows, f'{name}: CSV and JSON differ')
        for row in rows:
            require(all(1 <= int(s) <= 27 for s in row['spec_sections'].split(',')),
                    row['id'] + ': invalid section reference')
            for field in ('source_ids', 'reference_ids'):
                if field in row:
                    ids = [x for x in row[field].split(',') if x]
                    require(set(ids) <= ref_ids, row['id'] + ': unknown source')
                    expected_urls = ' ; '.join(next(r['url'] for r in refs if r['id'] == i) for i in ids)
                    require(row['source_urls'] == expected_urls, row['id'] + ': source URL mismatch')
        generated = {r['id']: r for r in rows}
        require(len(source[name]) == expected, name + ': canonical count differs')
        derived = {'source_urls', 'test_family_candidates', 'priority', 'dependency_gates'}
        for original in source[name]:
            require(original['id'] in generated, 'Missing canonical row ' + original['id'])
            for key, value in original.items():
                if key not in derived:
                    require(generated[original['id']][key] == value,
                            original['id'] + ': canonical field drift ' + key)
    tests = {r['id'] for r in planning['acceptance_tests']}
    tasks = {r['id']: r for r in planning['implementation_backlog']}
    for task in tasks.values():
        require(task['status'] == 'Not started', task['id'] + ': production work cannot be marked executed')
        require(task['dependency_gates'] == GATES[task['milestone']], task['id'] + ': wrong milestone gate')
        expected_priority = 'P2' if task['milestone'] == 'M8' else ('P0' if task['milestone'] in ('M0','M1','M2','M3','M7') else 'P1')
        require(task['priority'] == expected_priority, task['id'] + ': wrong priority')
        for field in ('acceptance_test_ids', 'test_family_candidates'):
            require(set(filter(None, task.get(field, '').split(';'))) <= tests,
                    task['id'] + ': invalid acceptance reference')
    for row in planning['acceptance_tests']:
        require(row['execution'] == 'Required in production implementation; not claimed executed here',
                row['id'] + ': incorrect execution claim')
    for task_id, milestone in {'N060':'M1','N064':'M1','N041':'M2','N054':'M2','N057':'M2',
                               'N059':'M2','N061':'M2','N062':'M2','N065':'M2','N043':'M3','N063':'M3'}.items():
        require(tasks[task_id]['milestone'] == milestone, task_id + ': first-class graph gate regressed')
    checks += ['62 source IDs unique and preserved', 'planning counts, IDs, source and test references resolved',
               'canonical planning matches CSV/JSON exports', 'M1/M2/M3 graph gates enforced',
               'production task and acceptance execution claims remain unexecuted']

    report = (ROOT / 'ENGINEERING_SPEC.md').read_text(encoding='utf-8')
    section_numbers = [int(x) for x in re.findall(r'^## (\d+)\.', report, re.M)]
    require(section_numbers == list(range(1, 28)), 'Main report must retain sections 1 through 27')
    for filename in ('ENGINEERING_SPEC.md', 'SPATIAL_REPLICATION.md', 'PROTOCOL_ADDENDUM.md'):
        text = (ROOT / filename).read_text(encoding='utf-8')
        used = set(re.findall(r'\[(S\d{2})\]', text))
        require(used <= ref_ids, filename + ': unknown cited source')
    for filename, number, title in [('SPATIAL_REPLICATION.md',17,'Fortnite / Unreal Replication Graph spatial foundation'),
                                    ('PROTOCOL_ADDENDUM.md',27,'Protocol completion contracts')]:
        start = report.index(f'## {number}. ')
        end = report.index(f'## {number+1}. ', start) if number < 27 else report.index('## Sources', start)
        require(source_links_local(report[start:end]).strip() == embedded_source(filename, number, title),
                filename + ': canonical section is not identical to embedded section')
    checks += ['27 engineering sections retained', 'canonical spatial and protocol text identical to embedded report',
               'all report source references resolve']

    pages = {}
    for file in ROOT.glob('*.html'):
        parser = Links()
        parser.feed(file.read_text(encoding='utf-8'))
        require(not parser.duplicates, file.name + ': duplicate HTML anchor')
        pages[file.name] = parser
    checked_links = 0
    for filename, parser in pages.items():
        for href in parser.links:
            url = urlsplit(href)
            if url.scheme or url.netloc:
                continue
            target = unquote(url.path) or filename
            require((ROOT / target).is_file(), filename + ': broken local file link ' + href)
            if url.fragment and target in pages:
                require(unquote(url.fragment) in pages[target].ids,
                        filename + ': broken cross-file or local anchor ' + href)
            checked_links += 1
    checks.append('HTML package links, source anchors and local anchors resolved')

    config = tomllib.loads((ROOT / 'config/arena_profile.toml').read_text(encoding='utf-8'))
    scale = tomllib.loads((ROOT / 'config/spatial_scale_profile.toml').read_text(encoding='utf-8'))
    interest, grid = config['interest'], config['interest']['grid']
    require(interest['model'] == 'epic_replication_graph', 'Wrong spatial basis')
    require(interest['required_from_milestone'] == 'M1', 'Graph gate not M1')
    require(not interest['production_broadcast_fallback'], 'Production broadcast fallback enabled')
    require(interest['prepare_once_per_replication_frame'] and interest['prepare_from_committed_world'], 'Unsafe prepare policy')
    require(interest['required_dependencies_override_distance'] and not interest['required_dependencies_override_disclosure'],
            'Dependency authorization rule changed')
    require(interest['unauthorized_dependency_policy'] == 'deny_prediction_group', 'Unsafe unauthorized dependency policy')
    require(0 < interest['max_required_dependency_entities'] <= interest['max_candidates_per_connection'] <= interest['max_known_scopes_per_connection'],
            'Inconsistent candidate/dependency/known-scope limits')
    require(interest['max_required_dependency_entities'] <= config['limits']['max_total_predicted_entities'], 'Required entity cap exceeds prediction cap')
    scope = interest['scope']
    require(scope['full_baseline_on_entry'] and scope['full_baseline_on_representation_change'] and scope['delta_requires_scope_ready_ack'],
            'Entry readiness policy missing')
    require(not scope['exit_is_authoritative_death'] and scope['state_and_control_validate_scope_epoch'], 'Unsafe scope lifecycle')
    require(interest['dormancy']['completion'] == 'per_connection_decoded_state_version', 'Dormancy lacks per-connection completion')
    require(not interest['dormancy']['periodic_broadcast_all_dormant'], 'Dormancy broadcast enabled')
    require(0 < interest['scheduler']['critical_budget_fraction'] < 1, 'Invalid reserve fraction')
    require(config['prediction']['max_replay_ticks_per_frame'] < config['prediction']['history_ticks'], 'Replay budget conflated with history')
    require(config['combat']['hit_history_ticks'] * 1000 / config['simulation']['hz'] > config['combat']['max_rewind_ms'], 'Hit history too short')
    require(config['limits']['max_prediction_history_bytes'] >= config['prediction']['history_ticks'] * config['limits']['max_predicted_groups'] * config['limits']['max_decoded_group_checkpoint_bytes'],
            'Worst-case checkpoint history exceeds memory cap')
    require(scale['scale']['replication_model'] == interest['model'], 'Scale/arena basis differs')
    require(scale['scale']['connections'] == config['qualification']['large_world_client_count'] == 100, 'Scale connection count mismatch')
    require(sum(c['count'] for c in scale['actor_classes'].values()) == scale['scale']['potential_replicated_actors'] == config['qualification']['large_world_potential_actor_count'] == 50000,
            'Scale class counts do not sum to actor total')
    membership_estimate = 0
    for name, actor_class in scale['actor_classes'].items():
        radius = actor_class['cull_radius_m'] + actor_class['bound_radius_m'] + grid['leave_margin_m'] + grid['prefetch_cap_m']
        require(radius >= 0 and grid['cell_size_m'] > 0, 'Invalid geometry defaults')
        cells = (ceil(2 * radius / grid['cell_size_m']) + 1) ** 2
        require(cells <= grid['max_cells_per_entity'], name + ': class exceeds grid membership cap')
        membership_estimate += cells * actor_class['count']
    require(membership_estimate <= grid['max_total_memberships'], 'Scale classes exceed conservative membership cap')
    checks += ['both TOML profiles parsed with cross-field constraints', 'scope/privacy/dormancy safety defaults enforced',
               'history, replay and checkpoint memory budgets checked', '50,000-actor class counts and conservative spatial membership budget checked']

    # Validate exporter determinism without needing any third-party packages.
    generated = [ROOT/'planning'/f'{name}.{ext}' for name in EXPECTED_COUNTS for ext in ('json','csv')]
    before = {str(p): digest(p) for p in generated}
    rebuilt = subprocess.run([sys.executable, str(ROOT/'build_planning.py')], cwd=ROOT, capture_output=True, text=True, timeout=30)
    require(rebuilt.returncode == 0, 'Planning rebuild failed: ' + rebuilt.stderr)
    require(before == {str(p): digest(p) for p in generated}, 'Planning rebuild changed exports; rerun validation')
    checks.append('planning exporter idempotent')

    completed = subprocess.run([sys.executable, '-m', 'unittest', 'discover', '-s', 'reference_model', '-v'],
                               cwd=ROOT, capture_output=True, text=True, timeout=60)
    output = completed.stdout + completed.stderr
    (ROOT / 'reference_model/TEST_RESULTS.txt').write_text(output, encoding='utf-8')
    match = re.search(r'Ran (\d+) tests? in ', output)
    require(completed.returncode == 0 and bool(match) and '\nOK\n' in output, 'Contract tests failed; inspect TEST_RESULTS.txt')
    count = int(match[1])
    require(count == 89, 'Unexpected contract test count; update revision metadata before delivery')
    checks.append('89 contract tests passed in this validation run')
    browser = load_json('BROWSER_CHECKS.json')
    browser_checks = browser['automated_checks']
    require(len(browser_checks) == 6, 'Incomplete recorded browser check matrix')
    require(all(r['width'] >= r['scrollWidth'] and not r['brokenLocalAnchors'] for r in browser_checks), 'Recorded browser check failure')
    checks.append('recorded desktop/mobile browser checks pass; review scope retained separately')

    result = {
        'revision': 2,
        'spatial_basis': 'Epic publicly documented Fortnite/Unreal Replication Graph; portable details explicitly derived',
        'engineering_sections': 27,
        'report_word_count_whitespace': len(report.split()),
        'primary_source_entries': len(refs),
        'implementation_tasks': len(tasks),
        'acceptance_cases': len(tests),
        'feature_requirements': len(planning['feature_inventory']),
        'contract_tests_passed': count,
        'original_contract_tests': 42,
        'new_spatial_scope_contract_tests': count - 42,
        'randomized_delivery_seeds': 100,
        'randomized_delivery_ticks_per_seed': 80,
        'spatial_oracle_seeds': 25,
        'spatial_oracle_actors_per_seed': 200,
        'spatial_oracle_update_query_rounds_per_seed': 20,
        'scope_message_permutations': 120,
        'python_version': platform.python_version(),
        'checked_local_html_links': checked_links,
        'conservative_scale_membership_estimate': membership_estimate,
        'checks': checks,
        'browser_review': browser,
        'not_executed': [
            '156 production acceptance cases', 'production implementation tasks', 'real network transport',
            '3D collision or game rendering', 'cross-platform qualification',
            '100-client/50,000-potential-actor production benchmark', '24-hour soak',
            'production field authorization/occlusion implementation', 'commercial-game comparative benchmark'
        ],
        'production_status': 'Engineering handoff and executable contract models; not a production client/server library',
        'validation_scope': 'Local consistency and contract execution only; web URLs are recorded research sources, not freshly probed by this validator.'
    }
    (ROOT / 'VALIDATION.json').write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    all_files = sorted(p for p in ROOT.rglob('*') if p.is_file() and '__pycache__' not in p.parts
                       and p.suffix != '.pyc' and p.name != 'SHA256SUMS.txt')
    (ROOT / 'SHA256SUMS.txt').write_text(''.join(f'{digest(p)}  {p.relative_to(ROOT).as_posix()}\n' for p in all_files), encoding='utf-8')
    return result


if __name__ == '__main__':
    try:
        result = validate()
    except (ValueError, OSError, KeyError, subprocess.TimeoutExpired) as exc:
        print(f'VALIDATION FAILED: {exc}', file=sys.stderr)
        raise SystemExit(1)
    print(f"Validated revision {result['revision']}: {result['implementation_tasks']} tasks, "
          f"{result['acceptance_cases']} acceptance cases, {result['feature_requirements']} requirements, "
          f"{result['primary_source_entries']} sources; {result['contract_tests_passed']} contract tests passed.")
