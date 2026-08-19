# OpenCode experiment control

`oc-run` prepares an isolated experiment workspace and starts the external OpenCode TUI.
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

`pull` returns every currently runnable artifact owned by the role. With no runnable work it waits
up to 60 seconds, then returns the pending artifacts and their `blocked_by` dependencies; the role
calls `pull` again. `submit` checks ownership, current inputs, and declared output checks before
atomically touching each artifact marker. The wildcard command permission does not grant wildcard
ownership: the DAG rejects every artifact not owned by the caller.

The Host publishes reviewed inputs and promotions with:

```text
oc-ctl publish <exec-name> <artifact...>
```

An `input` ending in `?` is optional. Its absence never blocks the first run; once published, its
mtime participates in freshness exactly like any other input. Required inputs must be current.
When an input becomes newer, dependent artifacts become stale and are returned by `pull` again.

Actual output files are only existence/nonempty checks. They never drive the DAG. All workflow
state is reconstructed from `control/artifacts/*` mtimes; there are no claims, generations, done
records, or special feedback semantics. File locking and atomic replacement serialize concurrent
publication.

`oc-ctl start` publishes `start_artifacts` once. `oc-ctl finish` requires the Host-owned
`finish_artifact` and every role-owned artifact to be current, writes `stop_path`, and waits for role
loops to observe `stopped: true`.
