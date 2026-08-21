# OpenCode experiment control

`oc-run <test-id> <port>` starts a persistent headless OpenCode daemon in an empty runner workspace,
thereby occupying the requested loopback port before any experiment material exists. The external
operator selects the port but never selects a plan. Early in a long-running task, the Host runs
`oc-ctl test-connect <test-id>` to exercise that exact daemon and record
`target/exp/<test-id>/connect-test.json`. This neither chooses a plan nor writes `config.json`, so it
does not release `oc-run` or freeze experiment inputs. After prerequisite fixes are complete, the
Host runs `oc-ctl start <test-id>` from inside its autonomously selected plan directory; `start`
requires the successful receipt and external runner, adopts its port, and atomically writes
`target/exp/<test-id>/config.json`. The runner then prepares the isolated experiment workspace,
creates the formal session on the existing daemon, and attaches the TUI. When the TUI exits,
`oc-run` terminates the daemon.
`oc-ctl` controls and observes the execution. A plan may define an artifact DAG in
`experiment.json.workflow`; `task_cli.py` is copied into the workspace as `bin/oc-task`.

## Artifact workflow

The schema is `telora.opencode-artifact-workflow/v1`. An artifact named `name.<role>` is owned by
that role. An artifact without a role suffix is Host-owned. Roles receive only two workflow
permissions:

```text
bin/oc-task pull <role>
bin/oc-task submit <role> <artifact...>
```

`pull` returns only the first runnable artifact owned by the role, using artifact declaration order.
A task starts when this pull succeeds and ends when that one artifact is submitted. The role pulls
again to claim its next runnable artifact. With no runnable work it waits up to 60 seconds and the
role immediately pulls again. Every role runs this loop for the whole external TUI lifetime; idle,
timeout, and submit are not exit conditions. Tasks are deliberately not merged.

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
oc-ctl start <test-id>
oc-ctl stat <test-id>
oc-ctl status <test-id>
oc-ctl update <test-id> <dest-file>=<src-file>...
oc-ctl publish <test-id> <artifact>[=!]...
```

`update` atomically copies any Host-readable file. Relative source paths are resolved from the
Host's current directory; absolute paths, including files under `/tmp`, are accepted. Destination
paths remain relative to the experiment workspace. A source of `!` deletes the destination.
`publish` touches a Host-owned artifact, or removes it when suffixed with `=!`. `status` combines
artifact and Agent state with the latest task elapsed time and tokens. `stat` reports each role/task
duration and tokens, longest thinking interval, and Telora command count.

An `input` ending in `?` is optional. Its absence never blocks the first run; once published, its
mtime participates in freshness exactly like any other input. Required inputs must be current.
When an input becomes newer, dependent artifacts become stale and are returned by `pull` again.

Actual output files are only existence/nonempty checks. They never drive the DAG. All workflow
state is reconstructed from `control/artifacts/*` mtimes. `.oc-task` records only observation
windows; it never becomes scheduling state. File locking and atomic replacement serialize
concurrent publication.

`oc-ctl start` publishes `start_artifacts` once and prompts the coordinator once. The coordinator
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
