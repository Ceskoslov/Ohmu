#!/usr/bin/env python3
"""Run the deterministic scale fixture; retain raw manifests outside source control."""
import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import platform
import resource
import subprocess

ROOT = Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('directory', type=Path)
p.add_argument('--binary', type=Path, default=ROOT / 'target/release/examples/scale')
p.add_argument('--sources', type=int, default=10000)
p.add_argument('--iterations', type=int, default=100)
p.add_argument('--prepared', action='store_true', help='Measure an already prepared corpus')
a = p.parse_args()
if a.sources < 10 or a.iterations < 8:
    p.error('at least 10 sources and eight query iterations required')
exe = a.binary.resolve(strict=True)
root = a.directory.resolve()
def run(mode, *extra, capture=False):
    return subprocess.check_output([str(exe), mode, str(root), *map(str, extra)], text=True) if capture else subprocess.run([str(exe), mode, str(root), *map(str, extra)], check=True)
if not a.prepared:
    run('generate', a.sources)
    run('prepare')
cold = [json.loads(run('cold', i, capture=True)) for i in range(8)]
report = json.loads(run('measure', a.iterations, capture=True))
values = sorted(x['ms'] for x in cold)
report['process_cold_ms'] = {'samples': cold, 'p50': values[(len(values)-1)*50//100], 'p95': values[math.ceil(len(values)*.95)-1]}
report['hardware'] = {'platform': platform.platform(), 'logical_cpus': os.cpu_count(), 'machine': platform.machine(), 'physical_storage': 'not identified by virtual guest'}
if Path('/proc/cpuinfo').exists():
    report['hardware']['cpu_model'] = next(l.split(':', 1)[1].strip() for l in Path('/proc/cpuinfo').read_text().splitlines() if l.startswith('model name'))
    report['hardware']['mem_total'] = next(l for l in Path('/proc/meminfo').read_text().splitlines() if l.startswith('MemTotal:'))
report['hardware']['filesystem'] = subprocess.check_output(['df', '-T', str(root)], text=True) if platform.system() == 'Linux' else subprocess.check_output(['df', str(root)], text=True)
report['peak_child_rss'] = {'value': resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss, 'unit': 'bytes' if platform.system() == 'Darwin' else 'KiB', 'scope': 'maximum across fixture child processes; excludes compiler'}
report['revision'] = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
report['dirty'] = bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT))
report['measured_at'] = datetime.now(timezone.utc).isoformat()
report['model_latency'] = None
(root / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
