"""Sequential, isolated initialization samples; run after builds and tests finish."""
import argparse
import json
import pathlib
import re
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument('--probe', required=True)
parser.add_argument('--output', required=True)
parser.add_argument('--samples', type=int, default=3)
parser.add_argument('artifacts', nargs='+', help='LABEL=PATH')
args = parser.parse_args()
records = []
for sample in range(args.samples):
    for item in args.artifacts:
        label, path = item.split('=', 1)
        start = time.monotonic()
        result = subprocess.run([args.probe, path, '64', '32', '64', '250000'],
                                text=True, capture_output=True, check=True)
        fields = dict(re.findall(r'(\w+)=([\d.]+)', result.stderr))
        record = {'label': label, 'sample': sample + 1, 'artifact': path,
                  'process_seconds': time.monotonic() - start,
                  **{k: float(v) if '.' in v else int(v) for k, v in fields.items()}}
        records.append(record)
        pathlib.Path(args.output).write_text(json.dumps(records, indent=2) + '\n')
        print(json.dumps(record), flush=True)
