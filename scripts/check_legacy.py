#!/usr/bin/env python3
"""Exercise historical producers against today's reader using isolated archives."""
import argparse
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary', type=Path, default=ROOT / 'target/debug/ohmu')
p.add_argument('--revision', action='append', help='Override historical producer commits')
a = p.parse_args()
current = a.binary.resolve(strict=True)
revisions = a.revision or ['d53bd10', 'eaf7473', 'eb17d54', '3a53b28']
env = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / 'target/legacy-producer'))
results = []
def invoke(binary, *args):
    return json.loads(subprocess.check_output([str(binary), '--json', *map(str, args)], text=True))['result']
with tempfile.TemporaryDirectory(prefix='ohmu-legacy-') as directory:
    directory = Path(directory)
    for revision in revisions:
        sha = subprocess.check_output(['git', 'rev-parse', '--verify', revision + '^{commit}'], cwd=ROOT, text=True).strip()
        checkout = directory / revision
        checkout.mkdir()
        archive = subprocess.check_output(['git', 'archive', '--format=tar', sha], cwd=ROOT)
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(checkout, filter='data')
        # Git archive uses historical mtimes. Force source freshness when sharing
        # the isolated build cache across same-name/version historical packages.
        for source_file in checkout.rglob('*.rs'):
            source_file.touch()
        subprocess.run(['cargo', 'build', '-p', 'ohmu-cli', '--manifest-path', str(checkout / 'Cargo.toml'), '--locked', '--offline'], cwd=ROOT, env=env, check=True)
        producer = ROOT / 'target/legacy-producer/debug/ohmu'
        wiki = directory / f'wiki-{revision}'
        invoke(producer, 'init', wiki)
        help_text = subprocess.check_output([producer, '--help'], text=True)
        before = invoke(producer, 'read', 'welcome', '--wiki', wiki)
        if 'ingest' in help_text:
            source = directory / f'input-{revision}.md'
            source.write_text('# Legacy evidence\n\nHistorical 中文 source remains exact.\n')
            invoke(producer, 'ingest', source, '--wiki', wiki)
        if 'snapshot' in help_text:
            proposal = directory / f'proposal-{revision}.json'
            proposal.write_text(json.dumps({'summary': 'Legacy draft', 'pages': [{'page_id': 'legacy', 'expected_revision': None, 'content': '# Legacy page\n', 'citations': []}]}))
            draft = invoke(producer, 'stage', proposal, '--wiki', wiki)
            invoke(producer, 'apply', draft['id'], '--wiki', wiki)
            snapshot = invoke(producer, 'snapshot', '--wiki', wiki)
            backup = directory / f'backup-{revision}'
            invoke(producer, 'backup', backup, '--wiki', wiki)
            restored = directory / f'restored-{revision}'
            invoke(current, 'restore', backup, restored)
            assert invoke(current, 'draft', draft['id'], '--wiki', restored) == draft
            assert invoke(current, 'snapshot-read', snapshot['id'], 'legacy', '--wiki', restored)['content'] == '# Legacy page\n'
        invoke(current, 'doctor', '--wiki', wiki)
        assert invoke(current, 'read', 'welcome', '--wiki', wiki) == before
        invoke(current, 'reindex', '--wiki', wiki)
        assert invoke(current, 'search', 'Welcome', '--wiki', wiki)['hits']
        sources = invoke(current, 'sources', '--wiki', wiki)['sources']
        for record in sources:
            assert 'Historical 中文' in invoke(current, 'source', record['source_id'], '--revision', record['revision'], '--wiki', wiki)['content']
        results.append({'producer': sha, 'read_reindex_search': 'passed', 'source_revisions': len(sources), 'durable_state_roundtrip': 'passed' if 'snapshot' in help_text else 'not applicable'})
print(json.dumps({'schema_version': 1, 'compatibility': results}, indent=2))
