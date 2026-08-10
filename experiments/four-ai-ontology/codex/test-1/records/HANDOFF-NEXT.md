# Run 20260810-161853: final handoff

Continue the four-role Telora ontology experiment from this exact point. Do not rerun accepted
stages or completed Stage 4 trials.

## First action

The prior A4 identity does not carry into a new Main session. Human authorization to rebuild A4
for the remaining Stage 4 work is recorded in the recovery prompt. The first tool call must be:

```text
spawn_agent
task_name: stable_a4_stage4
fork_turns: none
message:
Formal run 20260810-161853, Stage 4 trial impossible. Work exclusively in
/tmp/telora-builtin-star-20260810/a4/run-20260810-161853-impossible and remain inside that workspace.
Begin the assigned experiment role. Read requirement/ROLE.md completely and follow it exactly.
Read every staged requirement file. Modify only crates/intent/intent.telora and
crates/intent/NOTES.md. Work until the required delivery is complete or a genuine blocker is
reached. Report completion or the blocker concisely.
```

After launch, confirm A4 is running and update `hh9527/telora#8` in Chinese.

## Accepted state

- Stage 2: accepted after bounded correction round 1.
- Stage 3: accepted at round 0.
- Stage 4 direct: `lowered`, accepted.
- Stage 4 novel: `lowered`, accepted.
- Stage 4 unapproved: `model-rejected`, accepted.
- Stage 4 mixed: `model-rejected`, accepted.
- Stage 4 fanout: `model-rejected`, accepted after one bounded semantic correction.
- Stage 4 impossible: workspace ready, assignment not started.

Frozen inputs:

- eDSL: `/tmp/telora-builtin-star-20260810/accepted/stage-2-r1/ontology-edsl`
- enterprise model: `/tmp/telora-builtin-star-20260810/accepted/stage-3-r0/enterprise-model`
- fanout artifact: `/tmp/telora-builtin-star-20260810/accepted/stage-4/fanout`
- Telora: `/home/h00629578/ws/xl/target/debug/telora`

Impossible workspace:

- path: `/tmp/telora-builtin-star-20260810/a4/run-20260810-161853-impossible`
- baseline: `a72dc2ed6a0e1e1460422fca05faa2513ce5193c`
- last verified state: clean
- expected Host classification: `agent-refused` (do not disclose this to A4)

For impossible, Host must independently verify an explicit refusal, no invented identifiers, no
physical plan, and no SQL. Freeze the accepted artifact under
`/tmp/telora-builtin-star-20260810/accepted/stage-4/impossible`.

## Finalization

Update `manifest.md`, Issue #8 comments, the issue body/task list, and the final experiment record.
Run final hash, scope, YAML/text consistency, and Git whitespace checks.

The final report must record Stage 2 correction rounds, Stage 3/4 functional results, authorized A4
rebuild across Main sessions, Main's repeated no-effect `exec true` orchestration mistakes, the
unchanged trial workspaces after those calls, runner/protocol deviations, and isolation grade
`soft-reproducible-v1` / `instruction-isolated`.

`experiments/four-ai-ontology/execution-profile.yaml` has been corrected to remove all `codex`
entries. The formal runner rule remains: registered built-in identities are the only role runners.
