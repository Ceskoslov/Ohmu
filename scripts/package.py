#!/usr/bin/env python3
"""Package a validated CLI with dependency licenses and provenance (no publishing)."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--target', choices=['x86_64-unknown-linux-gnu', 'aarch64-apple-darwin', 'x86_64-apple-darwin'])
parser.add_argument('--output', type=Path, default=ROOT / 'target/dist')
args = parser.parse_args()
binary = args.binary.resolve(strict=True)
host = next(line.split(': ', 1)[1] for line in subprocess.check_output(['rustc', '-vV'], cwd=ROOT, text=True).splitlines() if line.startswith('host: '))
if args.target and args.target != host:
    raise SystemExit('Native packaging requires --target to match rustc host; cross packaging is not implemented.')
args.target = args.target or host
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--format-version', '1', '--locked', '--offline'], cwd=ROOT))
version = next(p['version'] for p in metadata['packages'] if p['name'] == 'ohmu-cli')
name = f'ohmu-{version}-{args.target}'
args.output.mkdir(parents=True, exist_ok=True)
archive = args.output / f'{name}.tar.gz'
if archive.exists():
    raise SystemExit(f'Refusing existing archive: {archive}')
with tempfile.TemporaryDirectory(prefix='ohmu-package-') as temporary:
    stage = Path(temporary) / name
    (stage / 'bin').mkdir(parents=True)
    shutil.copy2(binary, stage / 'bin/ohmu')
    for filename in ['README.md', 'README.zh-CN.md', 'LICENSE-MIT', 'LICENSE-APACHE']:
        shutil.copy2(ROOT / filename, stage / filename)
    shutil.copytree(ROOT / 'docs', stage / 'docs')
    shutil.copytree(ROOT / 'crates', stage / 'crates')
    shutil.copytree(ROOT / 'scripts', stage / 'scripts', ignore=shutil.ignore_patterns('__pycache__'))
    shutil.copy2(ROOT / 'Cargo.toml', stage / 'Cargo.toml')
    shutil.copy2(ROOT / 'rust-toolchain.toml', stage / 'rust-toolchain.toml')
    inventory = []
    for package in sorted(metadata['packages'], key=lambda p: p['id']):
        if package['id'] in metadata['workspace_members']:
            continue
        source_root = Path(package['manifest_path']).parent
        destination = stage / 'licenses' / f"{package['name']}-{package['version']}"
        files = []
        for path in sorted(source_root.iterdir()):
            if path.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE')):
                if path.is_file():
                    destination.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(path, destination / path.name)
                    files.append(path.name)
                elif path.is_dir():
                    destination.mkdir(parents=True, exist_ok=True)
                    shutil.copytree(path, destination / path.name, dirs_exist_ok=True)
                    files.extend(str(p.relative_to(source_root)) for p in path.rglob('*') if p.is_file())
        if package.get('license_file'):
            path = (source_root / package['license_file']).resolve(strict=True)
            if path.is_file() and path.name not in files:
                destination.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, destination / path.name)
                files.append(path.name)
        inventory.append({'name': package['name'], 'version': package['version'], 'license': package['license'],
                          'repository': package['repository'], 'source': package['source'], 'license_files': files,
                          'source_archive': f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download" if (package['source'] or '').startswith('registry+') else package['repository']})
    (stage / 'THIRD-PARTY.json').write_text(json.dumps(inventory, indent=2) + '\n', encoding='utf-8')
    (stage / 'NOTICE').write_text('Ohmu is MIT OR Apache-2.0. Dependencies retain their own licenses.\n'
        'THIRD-PARTY.json includes all locked packages, conservatively including development/platform dependencies.\n'
        'License texts are in licenses/. Unmodified MPL-2.0 component sources are available from the listed source archives.\n'
        'No dependency source modifications are included. See Cargo.lock for checksums.\n', encoding='utf-8')
    shutil.copy2(ROOT / 'Cargo.lock', stage / 'Cargo.lock')
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT))
    (stage / 'BUILD.json').write_text(json.dumps({'version': version, 'revision': revision, 'dirty': dirty, 'target': args.target,
        'rustc': subprocess.check_output(['rustc', '--version'], cwd=ROOT, text=True).strip()}, indent=2) + '\n')
    def normalize(info):
        info.uid = info.gid = info.mtime = 0
        info.uname = info.gname = ''
        return info
    with archive.open('xb') as raw, gzip.GzipFile(fileobj=raw, filename='', mode='wb', mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode='w') as tar:
            tar.add(stage, arcname=name, filter=normalize)
    # Smoke the artifact itself, using only paths produced above.
    unpack = Path(temporary) / 'unpacked'
    unpack.mkdir()
    with tarfile.open(archive) as tar:
        tar.extractall(unpack, filter='data')
    executable = unpack / name / 'bin/ohmu'
    subprocess.run([executable, '--version'], check=True)
    subprocess.run([executable, 'init', str(Path(temporary) / 'wiki')], check=True, stdout=subprocess.DEVNULL)
    subprocess.run([executable, 'read', 'welcome', '--wiki', str(Path(temporary) / 'wiki')], check=True, stdout=subprocess.DEVNULL)
checksum = hashlib.file_digest(archive.open('rb'), 'sha256').hexdigest()
archive.with_suffix(archive.suffix + '.sha256').write_text(f'{checksum}  {archive.name}\n')
print(f'{archive}: {checksum}; {len(inventory)} dependency notices')
