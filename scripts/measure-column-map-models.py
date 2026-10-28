#!/usr/bin/env python3
"""Compare baseline Dict and column Map/MapMut on unchanged, isolated model sources."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import time


def information(path):
    data = path.read_bytes()
    cursor = 8

    def word():
        nonlocal cursor
        value = shift = 0
        while True:
            byte = data[cursor]
            cursor += 1
            value |= (byte & 127) << shift
            if byte < 128:
                return value
            shift += 7

    while cursor < len(data):
        tag = data[cursor]
        cursor += 1
        size = word()
        end = cursor + size
        if tag == 0:
            size = word()
            name = data[cursor:cursor + size].decode()
            cursor += size
            if name == 'telora.build.information':
                return json.loads(data[cursor:end])
        cursor = end
    raise ValueError('missing build information')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--telora', type=Path, required=True)
    parser.add_argument('--baseline-telora',type=Path,required=True)
    parser.add_argument('--models', default='icloud_model,ns_model')
    parser.add_argument('--repeats', type=int, default=1)
    parser.add_argument('--first-backend', choices=['baseline', 'columns'], default='baseline')
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error('--repeats must be positive')
    workspace = args.workspace.resolve()
    binary = args.telora.resolve()
    models = args.models.split(',')
    revision = subprocess.check_output(['git', '-C', str(workspace), 'rev-parse', 'HEAD'], text=True).strip()
    base = Path(os.environ.get('TMPDIR') or Path.home() / '.cache/tmp')
    base.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='column-map-models-', dir=base))
    sources = output / 'workspace'
    sources.mkdir()
    members = ['ontology', *models]
    archive = subprocess.check_output(['git', '-C', str(workspace), 'archive', revision,
                                      *members, 'telora-config.json', 'telora-lock.json'])
    with tarfile.open(fileobj=io.BytesIO(archive)) as packed:
        packed.extractall(sources, filter='data')
    config_path = sources / 'telora-config.json'
    config = json.loads(config_path.read_text())
    config['members'] = members
    config_path.write_text(json.dumps(config, indent=2) + '\n')
    lock_path = sources / 'telora-lock.json'
    lock = json.loads(lock_path.read_text())
    lock['packages'] = {name: lock['packages'][name] for name in members}
    lock_path.write_text(json.dumps(lock, indent=2) + '\n')
    report = {'output': str(output), 'model_revision': revision,
              'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'runtime': config.get('runtime'), 'first_backend': args.first_backend,
              'baseline_sha256':hashlib.sha256(args.baseline_telora.read_bytes()).hexdigest(),
              'baseline_binary':str(args.baseline_telora.resolve()),
              'measurements': [], 'information_equal': {}}
    report_path = output / 'report.json'
    print(json.dumps({'output': str(output), 'model_revision': revision}), flush=True)
    for repeat in range(args.repeats):
        # Alternate order on subsequent rounds to reduce systematic order bias.
        backends = ['baseline', 'columns'] if args.first_backend == 'baseline' else ['columns', 'baseline']
        if repeat % 2:
            backends.reverse()
        for model in models:
            information_by_backend = {}
            for backend in backends:
                name = f'{model}-{backend}-{repeat + 1}'
                artifact = output / f'{name}.wasm'
                rss_file = output / f'{name}.time'
                env = {**os.environ, 'TELORA_EXPERIMENTAL_DICT_PD': '0'}
                command = ['/usr/bin/time', '-v', '-o', str(rss_file), str(binary if backend=='columns' else args.baseline_telora.resolve()),
                           '-C', str(sources / model), 'build', model, '--staged',
                           '-o', str(artifact), '--report-usage']
                started = time.monotonic()
                result = subprocess.run(command, text=True, capture_output=True, env=env, timeout=900)
                elapsed = time.monotonic() - started
                (output / f'{name}.log').write_text(result.stderr + result.stdout)
                records = []
                for line in result.stderr.splitlines():
                    if line.startswith('{'):
                        try:
                            records.append(json.loads(line))
                        except json.JSONDecodeError:
                            pass
                rss = next((int(line.rsplit(':', 1)[1].strip()) for line in rss_file.read_text().splitlines()
                            if 'Maximum resident set size' in line), None)
                item = {'model': model, 'backend': backend, 'repeat': repeat + 1,
                        'returncode': result.returncode, 'wall_seconds': elapsed,
                        'max_rss_kib': rss, 'artifact_bytes': artifact.stat().st_size if artifact.exists() else None,
                        'phases': {r['phase']: r['elapsed_seconds'] for r in records
                                   if r.get('code') == 'build-phase-timing'},
                        'usage': [r for r in records if r.get('record') == 'buildtime-usage']}
                if result.returncode == 0:
                    info = information(artifact)
                    information_by_backend[backend] = info
                    (output / f'{name}.information.json').write_text(json.dumps(info, ensure_ascii=False, indent=2) + '\n')
                else:
                    item['error_tail'] = result.stderr[-4000:]
                report['measurements'].append(item)
                report_path.write_text(json.dumps(report, indent=2) + '\n')
                print(json.dumps(item), flush=True)
            if len(information_by_backend) == 2:
                report['information_equal'][f'{model}-{repeat + 1}'] = (
                    information_by_backend['baseline'] == information_by_backend['columns'])
                report_path.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'report': str(report_path), 'information_equal': report['information_equal']}), flush=True)


if __name__ == '__main__':
    main()
