# Run the same eight requests against published ordinary or snapshot artifacts.
import argparse
import json
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--runner', required=True)
parser.add_argument('artifacts', nargs='+')
args = parser.parse_args()
requests = [{'method': 'ic/discovery', 'input': {}}]
requests += [
    {'method': 'ic/info', 'input': {'key': key}}
    for key in [
        'index', 'Ty/DatetimeUtc', 'Dataset/tenant', 'Dataset/onu_kpi',
        'Dimension/onu_kpi/onu_kpi__cpuUsage',
        'Dimension/current_alarm/alarm_tenant_id',
    ]
]
requests.append({'method': 'ic/transform', 'input': {'intents': [{
    'op': 'Graph', 'root': 't', 'nodes': [{'id': 't', 'entity': 'tenant'}],
    'edges': [], 'select': [{'node': 't', 'dimension': 'tenant_id'}],
}]}})
expected = None
for artifact in args.artifacts:
    result = subprocess.run([
        args.runner, artifact, '--serve', 'stdio+jsonl://',
        '--initialization-fuel', '250000', '--request-fuel', '100000',
        '--with-memory-limit', '64',
    ], input=''.join(json.dumps(r) + '\n' for r in requests),
        text=True, capture_output=True, timeout=600, check=True)
    replies = [json.loads(line) for line in result.stdout.splitlines()]
    assert len(replies) == len(requests), (artifact, result.stdout, result.stderr)
    assert all(not reply.get('error') for reply in replies), (artifact, replies)
    values = [reply['ok'] for reply in replies]
    if expected is None:
        expected = values
    assert values == expected, artifact
    print(json.dumps({'artifact': artifact, 'matching_requests': len(values)}))
