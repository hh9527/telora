# OpenCode experiment control

`oc-lab run <lab-name> [--port PORT]` owns one persistent headless OpenCode server and a temporary
lab root. Once healthy, it writes exactly `{port, root}` to
`target/labs/<lab-name>/config.json`. The port is selected automatically when omitted. Every
execution workspace and controller record created by `oc-ctl` lives below that root, so exiting
`oc-lab run` terminates the server and reclaims the complete laboratory.

Early in a long-running task, the Host runs `oc-ctl test-connect <lab-name>`. This exercises health
and session creation in the lab without choosing a plan. After prerequisite fixes, the Host runs
`oc-ctl start <lab-name> <plan-id>`. It allocates a deterministic OpenCode session title such as
`ontology-3/1`, prepares its workspace under the lab root, starts the session, and returns that title.
Later control commands identify an execution with the pair `<lab-name> <session-name>`.
OpenCode's `title` is the session name and the sole session-name registry. Titles contain no spaces.
Root executions use `<plan-id>/<generation>`; a thread named `0001` uses
`<plan-id>.0001/<generation>`; controller-created role replacements use
`<plan-id>.<role>/<generation>`. `oc-lab ls` queries these sessions directly, and
`oc-lab attach <lab-name> <session-name>` resolves an exact title before launching the TUI.
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
oc-ctl test-connect <lab-name>
oc-ctl start <lab-name> <plan-id>
oc-ctl start <lab-name> <plan-id> --from <earlier-session-name>
oc-ctl start <lab-name> <thread-service-plan> --bundle <bundle-directory>
oc-ctl stat <lab-name> <session-name>
oc-ctl status <lab-name> <session-name>
oc-ctl pull <lab-name> <session-name> [<since>] [--timeout 60]
oc-ctl event <lab-name> <session-name> <event-id>
oc-ctl update <lab-name> <session-name> <dest-file>=<src-file>...
oc-ctl update <lab-name> <session-name> <dest-file>=<src-file>... --force
oc-ctl publish <lab-name> <session-name> <artifact>[=!]...
oc-ctl publish <lab-name> <session-name> <artifact>[=!]... --force
oc-ctl resume <lab-name> <session-name> <role>
oc-ctl resume <lab-name> <session-name> <role> --force
oc-ctl abort-sessions <lab-name> <session-name>
oc-ctl approve-baseline <lab-name> <session-name> <role>
oc-ctl open-thread <lab-name> <session-name> <role> <thread-name> <problem-file>
oc-ctl comment-thread <lab-name> <session-name> <role> <thread-name> <comment-file>
oc-ctl close-thread <lab-name> <session-name> <role>
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
`metrics.roles.<role>.commands`. `pull` returns immediately when a non-current Host-owned artifact's
artifact inputs are ready,
otherwise waits at most 60 seconds. At exit, its `events` contain every visible compact `thinking`,
`action`, `reply`, `task`, `artifact`, and `host_action` record whose Unix-millisecond `at >= since`.
Timeout is only a waiting bound, never an event upper bound. `next_since` is the greatest returned
`at`, or remains `since` when there are no events. Stable IDs let callers deduplicate the inclusive
boundary on the next pull.
`requests` is the ordered list of ready, non-current Host artifacts used by mandatory inputs (and
the finish artifact). `opt_requests` contains the remaining ready Host artifacts, including inputs
used only through `artifact?`. Both are current snapshots and are never filtered by `since`; file
checks remain publication-time validation. The first mandatory request snapshot and later
`requests` changes wake `pull` immediately. `opt_requests` never wakes it, while both unchanged
snapshots remain visible in the response without causing a busy loop.
Pass `next_since` into the next call, and use `event` with
an event ID for sanitized detail. Metric patterns that match no files and missing configured work boundaries are
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

`abort-sessions` retires an execution's live session tree without stopping the lab. It
recursively finds the coordinator and child sessions, aborts only active turns, and preserves all
session history, workspace files, artifacts, and the `oc-lab` daemon. Repeating it after the tree is
idle is a no-op.

A plan with `execution.kind = thread-service` runs its declared role as the root Agent and has no
coordinator or artifact pull loop. `start --bundle` installs only the paths declared by the plan,
rejects links and special files, and records a deterministic content digest. After the qualification
turn finishes, `approve-baseline` checks the declared outputs, runs the declared validation command,
and freezes the root session plus bundle digest. `open-thread` forks that baseline into a detached
session and delivers one UTF-8 problem file. `comment-thread` delivers clarification to that same
session. `close-thread` archives the completed session and releases the role for another problem.
Only one thread per role can be active, and a changed baseline session or bundle invalidates future
forks. `status`, `stat`, `event`, and `abort-sessions` include detached sessions from the controller's
thread registry; forked baseline history is excluded from per-thread metrics.

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

`oc-lab run` is an external-operator command. It owns the lab root and headless daemon lifecycle and is never part of
the Host permission preflight or Host scheduling command set. The Host runs `test-connect` while
authorization can still be attended and persistently approves the whole `oc-ctl` command prefix,
not only the probe's exact argv.

Before `oc-ctl start`, the Host must obtain every permission needed for the complete experiment.
From `start` until the experiment is fully accepted, the Host must never request temporary authorization: an
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
