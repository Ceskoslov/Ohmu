#!/usr/bin/env python3
"""Opt-in live evaluation against user-supplied held-out human annotations."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary', type=Path, default=ROOT / 'target/release/ohmu')
p.add_argument('--wiki', type=Path, required=True)
p.add_argument('--config', type=Path, required=True, help='Provider/model/counter/budget JSON; no keys')
p.add_argument('--cases', type=Path, required=True, help='JSON array: id, question, expected_sources, expected_abstain; annotations are never sent')
p.add_argument('--output', type=Path, required=True, help='New evaluation directory')
p.add_argument('--run-live', action='store_true', required=True, help='Explicitly authorize sending this dataset to the configured provider')
p.add_argument('--max-requests', type=int, default=20, help='Maximum questions, each bounded to 12 model requests')
a = p.parse_args()
config = json.loads(a.config.read_text())
allowed = {'endpoint', 'model', 'token_counter', 'context_window', 'output_tokens', 'key_env', 'no_api_key', 'input_usd_per_million', 'output_usd_per_million'}
if set(config) - allowed:
    p.error('unknown configuration fields; store API keys only in the environment')
cases = json.loads(a.cases.read_text())
if not isinstance(cases, list) or not 1 <= len(cases) <= a.max_requests:
    p.error('case count outside explicit evaluation budget')
if len({case['id'] for case in cases}) != len(cases):
    p.error('duplicate case IDs')
a.output.mkdir(parents=False, exist_ok=False)
results = []
for i, case in enumerate(cases):
    args = [str(a.binary.resolve(strict=True)), '--json', 'ask', case['question'], '--wiki', str(a.wiki.resolve()), '--events', str(a.output / f'query-{i:04}.jsonl')]
    for key in ['endpoint', 'model', 'token_counter', 'context_window', 'output_tokens']:
        args += ['--' + key.replace('_', '-'), str(config[key])]
    args += ['--no-api-key'] if config.get('no_api_key') else ['--key-env', config.get('key_env', 'OHMU_API_KEY')]
    process = subprocess.run(args, text=True, capture_output=True, check=False, timeout=150)
    if process.returncode:
        raise SystemExit(f'Case {case["id"]} failed; existing journals are retained. No automatic retry. Inspect stderr locally: {process.stderr}')
    report = json.loads(process.stdout)['result']
    answer = report.get('answer') or {}
    citations = [c for group in ['facts', 'synthesis', 'contradictions'] for claim in answer.get(group, []) for c in claim['citations']]
    found = {c['locator']['source_id'] for c in citations}
    expected = set(case.get('expected_sources', []))
    usage = report['usage']
    cost = None
    if {'input_usd_per_million', 'output_usd_per_million'} <= set(config):
        cost = (usage['input_tokens'] * config['input_usd_per_million'] + usage['output_tokens'] * config['output_usd_per_million']) / 1000000
    results.append({'id': case['id'], 'accepted': report['accepted'], 'partial': report['partial'], 'elapsed_ms': report['elapsed_ms'],
        'quote_checks_passed': report['accepted'], 'cited_sources': sorted(found), 'expected_source_recall': len(found & expected) / len(expected) if expected else None,
        'abstained': not any(answer.get(k) for k in ['facts', 'synthesis', 'contradictions']), 'expected_abstain': case.get('expected_abstain'),
        'usage': usage, 'estimated_cost_usd': cost, 'human_answer_support': None, 'human_contradiction_handling': None,
        'answer': answer, 'reason': report['reason']})
summary = {'schema_version': 1, 'config': config, 'cases_sha256': hashlib.sha256(a.cases.read_bytes()).hexdigest(),
    'results': results, 'semantic_quality': 'Requires independent human review; quote identity is not entailment.'}
(a.output / 'evaluation.json').write_text(json.dumps(summary, indent=2, ensure_ascii=False) + '\n')
print(f'Wrote {a.output / "evaluation.json"}; human support/contradiction judgments remain unscored.')
