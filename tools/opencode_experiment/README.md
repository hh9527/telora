# OpenCode experiment control

`oc-run <test-id> <port>` starts a persistent headless OpenCode daemon in an empty runner workspace,
thereby occupying the requested loopback port before any experiment material exists. The external
operator selects the port but never selects a plan. Early in a long-running task, the Host runs
`oc-ctl test-connect <test-id>` to exercise that exact daemon and record
`target/exp/<test-id>/connect-test.json`. This neither chooses a plan nor writes `config.json`, so it
does not release `oc-run` or freeze experiment inputs. After prerequisite fixes are complete, the
Host runs `oc-ctl start <test-id> <plan-id>` from the repository root; `plan-id` names a tracked
directory under `experiment-plans/`. `start` requires the successful receipt and external runner, adopts its port,
and atomically writes
`target/exp/<test-id>/config.json`. The runner then prepares the isolated experiment workspace,
creates the formal session on the existing daemon, and attaches the TUI. When the TUI exits,
`oc-run` terminates the daemon.
`oc-ctl` controls and observes the execution. The tracked plan is runtime-neutral: it declares
workspace inputs, role capabilities and the artifact DAG. `oc-ctl` deterministically generates
`opencode.json`, `.opencode/agents/*.md` and the runtime `experiment.json`; Host-only assets are not
copied unless explicitly delivered. `task_cli.py` is copied as `bin/oc-task`.

## Artifact workflow

The schema is `telora.artifact-workflow/v1`. An artifact named `name.<role>` is owned by
that role. An artifact without a role suffix is Host-owned. Roles receive only two workflow
permissions:

```text
bin/oc-task pull <role>
bin/oc-task submit <role> <artifact...>
```

`pull` returns only the first runnable artifact owned by the role, using artifact declaration order.
A task starts when this pull succeeds and ends when that one artifact is submitted. The role pulls
again to claim its next runnable artifact. With no runnable work it returns a wait record after at
most 60 seconds; the role must immediately pull again. `--timeout <seconds>` can only shorten
that heartbeat. Every role runs this loop for the whole external TUI lifetime; roles never stop
themselves and tasks are deliberately not merged.

Each returned output artifact includes `output_mtime_ns`. Every direct input includes its current
`mtime_ns`, `available`, and `changed`, where `changed` is computed without stored history:

```text
changed := input.mtime_ns > output_mtime_ns
```

An absent output has mtime zero, so every available input is changed on the first run. An absent
optional input has mtime zero and is not changed. Inputs are listed per output artifact rather than
as a merged task-wide set, so a role can see exactly why each output became runnable.

The Host control surface is deliberately limited to:

```text
oc-ctl test-connect <test-id>
oc-ctl start <test-id> <plan-id>
oc-ctl start <test-id> <plan-id> --from <earlier-test-id>
oc-ctl stat <test-id>
oc-ctl status <test-id>
oc-ctl pull <test-id> [<since-ns>] [--timeout 60]
oc-ctl update <test-id> <dest-file>=<src-file>...
oc-ctl update <test-id> <dest-file>=<src-file>... --force
oc-ctl publish <test-id> <artifact>[=!]...
oc-ctl publish <test-id> <artifact>[=!]... --force
oc-ctl resume <test-id> <role>
oc-ctl resume <test-id> <role> --force
```

`update` atomically copies any Host-readable file. Relative source paths are resolved from the
Host's current directory; absolute paths, including files under `/tmp`, are accepted. Destination
paths remain relative to the experiment workspace. A source of `!` deletes the destination.
`publish` touches a Host-owned artifact, or removes it when suffixed with `=!`. Ordinary `update`
rejects replacement of a file checked by a role-owned artifact. `--force` explicitly crosses that
ownership boundary while preserving safe paths, known artifact identities, DAG inputs and checks.
It marks affected active tasks stale and records an auditable Host intervention in execution state
and the archived workspace. `status` returns a
compact scheduling view with `complete`, `quiescent`, normalized Agent state, publishable artifacts,
and `next_host_actions`; `status --verbose` additionally includes the complete artifact graph and raw
runtime state. `stat` reports each role/task duration and tokens, longest thinking interval, and
the count and elapsed time of command categories declared by the plan under
`metrics.roles.<role>.commands`. `pull` returns immediately when a Host-owned artifact is publishable,
otherwise waits at most 60 seconds and summarizes task, artifact and intervention events newer than
`since-ns`; pass its `next_since_ns` into the next call. Metric patterns that match no files and missing configured work boundaries are
reported as warnings instead of silently producing authoritative-looking zeroes.

`resume` restores the permanent `pull -> work -> submit -> pull` loop. It is idempotent when a role
is already busy, first resumes the newest inactive session, and directly creates a child replacement
session when that session cannot return or is missing. It succeeds only after observing the role
busy in its loop; otherwise it times out. Historical and replacement sessions are aggregated as one
role in metrics while their session ids remain visible. `status --verbose` includes recent assistant
text responses so the Host can identify clarification boundaries. `resume --force` aborts the
role's current turn and creates a clean replacement child session. Use it when a
role is stuck or must reload a corrected runtime adapter; the active artifact task remains in the
DAG and is reclaimed by the replacement role.

`start --from` creates a fresh workspace and OpenCode session while inheriting trusted progress from
an earlier execution of the same plan. An artifact is inherited only when it is current and its
normalized definition is unchanged in the new plan. The sole compatibility exception is a Host
promotion strengthened with required prerequisites that were already current before that promotion;
required dependencies must be inherited too. Supplying `--from` is an explicit Host decision to
accept those old outputs even when current root files have changed; use a normal start when changed
language or requirements are intended to invalidate upstream work.
The checked files for those non-root artifacts are copied and their touch-files are rebuilt in DAG
order. Session context, `.oc-task` records, and old role state are never copied. New artifacts are
therefore runnable immediately, while unchanged completed stages remain current.

An `input` ending in `?` is optional. Its absence never blocks the first run; once published, its
mtime participates in freshness exactly like any other input. Required inputs must be current.
When an input becomes newer, dependent artifacts become stale and are returned by `pull` again.

Actual output files are only existence/nonempty checks. They never drive the DAG. All workflow
state is reconstructed from `control/artifacts/*` mtimes. `.oc-task` records only observation
windows; it never becomes scheduling state. File locking and atomic replacement serialize
concurrent publication.

`oc-ctl start` publishes absent `start_artifacts` once and prompts the coordinator once. Inherited
current roots are not touched again, so their completed downstream artifacts do not become stale.
The coordinator
starts every role once and exits; it never dispatches, retries, observes, or interprets work.

## Host scheduling contract

`oc-run` is an external-operator command. It owns the external TUI lifecycle and is never part of
the Host permission preflight or Host scheduling command set. The Host runs `test-connect` while
authorization can still be attended and persistently approves the whole `oc-ctl` command prefix,
not only the probe's exact argv.

Before `oc-ctl start`, the Host must obtain every permission needed for the complete experiment.
From `start` until the external TUI ends, the Host must never request temporary authorization: an
approval prompt must not suspend observation, artifact publication, file delivery, or any other
scheduling responsibility. If a capability was omitted, the Host continues the experiment using
already-authorized observation and records the infrastructure defect for correction afterward.

When progress must be reported to a GitHub issue, the Host must also verify `gh issue comment`
permission before `start`. Reporting is a side effect of observation, never a DAG dependency: a
failed, delayed, or rate-limited comment must not delay `status`, `update`, `publish`, or Agent work.
The Host keeps scheduling and reports the missed update later when that can be done without
interrupting the experiment.

`start` also validates every role's command permissions against `permission_preflight`: an allowed
`./bin/telora <subcommand>` or `./bin/oc-task <subcommand>` family that has no preflight representative
is rejected as stale plan configuration. This prevents removed CLI such as `telora types` from being
advertised to Agents without being noticed during preparation.
