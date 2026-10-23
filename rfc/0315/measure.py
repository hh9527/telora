"""Run after builds and tests finish; pair identical inputs and validate outputs."""
import argparse
import hashlib
import json
import pathlib
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument('--probe', required=True)
parser.add_argument('--root', required=True, type=pathlib.Path)
parser.add_argument('--output', required=True, type=pathlib.Path)
parser.add_argument('--samples', type=int, default=3)
parser.add_argument('--workloads', nargs='+', default=['empty', 'single', 'many', 'strings', 'ic'])
parser.add_argument('--heap-bytes', type=int)
parser.add_argument('--allocation-interval', type=int, default=16384)
args = parser.parse_args()
records = []
expected = {}
for sample in range(1, args.samples + 1):
    versions = ['baseline', 'candidate'] if sample % 2 else ['candidate', 'baseline']
    for workload in args.workloads:
        for version in versions:
            artifact = args.root / f'{version}-{workload}.wasm'
            requests = args.root / ('ic-requests.jsonl' if workload == 'ic' else 'micro-requests.jsonl')
            replies = args.root / f'{version}-{workload}-{sample}.replies.jsonl'
            start = time.monotonic()
            command = [args.probe, str(artifact), str(requests), str(replies)]
            if args.heap_bytes is not None:
                command += [str(args.heap_bytes), str(args.allocation_interval)]
            result = subprocess.run(command,
                                    text=True, capture_output=True, timeout=1800)
            stem = args.root / f'{version}-{workload}-{sample}'
            stem.with_suffix('.stdout').write_text(result.stdout)
            stem.with_suffix('.stderr').write_text(result.stderr)
            if result.returncode:
                raise RuntimeError(f'{stem}: {result.stderr}')
            outputs = [json.loads(line) for line in replies.read_text().splitlines()]
            assert len(outputs) == len(requests.read_text().splitlines()), stem
            assert all(not reply.get('error') for reply in outputs), stem
            assert outputs == expected.setdefault(workload, outputs), stem
            records.append({'sample': sample, 'version': version, 'workload': workload,
                            'artifact_sha256': hashlib.sha256(artifact.read_bytes()).hexdigest(),
                            'requests_sha256': hashlib.sha256(requests.read_bytes()).hexdigest(),
                            'process_seconds': time.monotonic() - start,
                            'measurements': [json.loads(line) for line in result.stdout.splitlines()]})
            args.output.write_text(json.dumps(records, indent=2) + '\n')
            print(f'{version} {workload} sample={sample} verified', flush=True)
