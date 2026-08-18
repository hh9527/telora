# OpenCode experiment control

`oc-run` prepares an isolated experiment workspace and starts the external OpenCode TUI.
`oc-ctl` controls and observes the execution. A plan may define a file-driven node DAG in
`experiment.json.workflow`; the standalone `task_cli.py` is copied into the workspace as
`bin/oc-task`.

## Node workflow

The schema is `telora.opencode-node-workflow/v1`. Node suffixes define ownership:

- `*.rc` is an Agent-owned task and candidate output. The task id and node id are identical.
- `*.ready` is a Host-owned release decision. Downstream work cannot begin before it is current.
- `*.feedback` is Host-owned content associated with one observed `.rc` version.

Output files are checks, not DAG drivers. A task becomes runnable when all `needs` nodes and `after`
tasks are current. `mark-done` requires the explicit `.rc` suffix, checks declared outputs, and
atomically publishes that same node; an Agent cannot publish `.ready` or `.feedback`.

```text
oc-ctl ready <exec-name> <node.ready>
oc-ctl feedback <exec-name> <node.feedback> --body-file <file>
```

`ready` rejects incomplete dependencies, checks, or required review tasks. `feedback` additionally
requires a current observed `.rc`, a nonempty body, and all configured review tasks. Its timestamp
invalidates the older `.rc`; after the Agent republishes `.rc`, that feedback becomes historical and
does not trigger another iteration. Feedback is optional, so its absence never blocks initial work.

The role loop is:

```text
bin/oc-task next <role>
bin/oc-task mark-done <role> <name.rc>
```

`next` atomically claims the first runnable task in manifest order and otherwise waits. Repeating it
returns the existing claim. A task may declare same-role tasks in `absorbs`. When both the parent and
an absorbed task are runnable, the parent is claimed and its response lists the absorbed obligations.
Calling `mark-done` for an absorbed `.rc` publishes it but retains the parent claim; the parent cannot
complete until every absorbed obligation is done. This lets a build report review completion during
the build without scheduling a redundant review pass.

`mark-done` is idempotent and rejects completion if inputs changed after the claim. `oc-ctl start`
publishes every `start_nodes` entry once. `oc-ctl finish` requires the
`finish_node`, every task, and all claims to be quiescent before writing `stop_path`; waiting roles
then return `stopped: true`.

State lives under `control/nodes/` and `.oc-task/` in the isolated workspace. Changes use file locks,
atomic replacement, and nanosecond mtimes. File watches may optimize wake-up later, but do not define
ordering semantics.
