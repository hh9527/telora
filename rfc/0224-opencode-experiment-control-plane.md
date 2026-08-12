# RFC 0224: Opencode Experiment Control Plane

- Status: Proposed
- Depends on: RFC 0217

## Summary

Replace the ontology eDSL experiment's plan-specific Shell controllers with a
repository-wide opencode experiment control plane composed of two commands:

```text
oc-run <plan-id> <exec-name>
oc-ctl <command> <exec-name> ...
```

An experiment plan is versioned under `experiments/<plan-id>/`. An execution is
identified independently under `target/exp/<exec-name>/`; its immutable `plan`
file contains exactly `<plan-id>` followed by one newline. This separation lets
one plan produce multiple comparable executions and lets several long-lived
AI-2, AI-3, and AI-4 sessions coexist.

`oc-run` is executed by the human in an external terminal. It creates or
resumes one isolated workspace and opencode session, starts the daemon, enters
the TUI, and owns the daemon/TUI lifecycle. It never submits the experiment
task.

`oc-ctl` is executed by the Main agent. It validates, starts, observes, queries,
messages, feeds back to, validates, and finalizes the external session without
owning its interactive process. A completed assistant `finish=stop` ends one
conversation round; only `oc-ctl finish` ends the execution.

The implementation uses Python 3.11 or newer and only the Python standard
library. User-supplied offline queries use an automatically selected `jaq` or
`jq`, either directly from `PATH` or through `mise x`.

The first mandatory migration is `experiments/ontology-edsl/`. Its current
`open-opencode-tui.sh`, `control-opencode.sh`, and `observe-opencode.sh`
capabilities move to `oc-run` and `oc-ctl`; the old scripts are deleted after
equivalence, recovery, and migration tests pass. Historical
`target/opencode-test-*` archives remain immutable.

## Motivation

The current ontology eDSL protocol proved that an external opencode TUI can
share one session with an observing Main agent. Its Shell scripts also exposed
the limits of a plan-specific controller:

- one global `target/exp/` identity prevents concurrent or named executions;
- state is distributed across several files without one validated state model;
- long `curl` and `jq` commands invite repeated permission prompts and quoting
  mistakes;
- preparation, session control, observation, validation, and archiving are
  coupled to ontology-specific filenames;
- the first `finish=stop` is treated as terminal, although a multi-stage
  experiment needs later questions and downstream feedback;
- completed data is not exposed through one stable live/offline query model;
- cleanup and archive steps remain manual; and
- extending the protocol to AI-2 through AI-4 would duplicate more Shell.

The next experiment family uses Main as a star-topology orchestrator. Main
communicates independently with external AI-2, AI-3, and AI-4 sessions, reads
their completed answers, and relays downstream feedback upstream. The roles do
not communicate directly. This requires several recoverable sessions, explicit
message provenance, feedback rounds, and a controller that remains reliable
under JSON, HTTP, locking, and filesystem failures.

## Goals

1. separate reusable experiment plans from named executions;
2. preserve the external TUI as the human-visible process and daemon owner;
3. give Main one stable command surface for all session operations;
4. support several simultaneous named executions from different plans;
5. support multiple ordinary questions and formal feedback rounds per session;
6. preserve raw and derived evidence before an execution is retired;
7. make the same query interface work while live and after shutdown;
8. make state transitions locked, atomic, validated, and recoverable;
9. use no third-party Python packages;
10. migrate the current ontology eDSL experiment completely; and
11. retain historical experiment archives without rewriting them.

## Non-goals

This RFC does not:

- provide an operating-system security sandbox;
- make AI roles communicate directly;
- decide the semantic content Main relays between roles;
- automatically accept downstream claims as upstream feedback;
- replace opencode, Telora, Git, `jaq`, `jq`, or mise;
- make the TUI a web frontend or split its daemon from its ordinary lifecycle;
- migrate immutable historical `target/opencode-test-*` directories;
- define the complete AI-2 through AI-4 research plan; or
- require a general workflow engine or asynchronous Python framework.

## Terminology

- **plan**: versioned experiment material and machine-readable policy under
  `experiments/<plan-id>/`;
- **execution**: one named instantiation of a plan;
- **exec-name**: permanent repository-local name of that execution;
- **workspace**: isolated temporary directory visible to the role;
- **round**: one user message followed by assistant work ending in a terminal
  assistant message;
- **initial round**: the plan's fixed first task;
- **ask round**: an ordinary Main-to-role follow-up;
- **feedback round**: a provenance-bearing downstream-feedback relay;
- **live document**: normalized query data collected from the daemon; and
- **frozen document**: the equivalent data persisted by `finish`.

## Command installation and layout

The repository provides executable Python entry points named `oc-run` and
`oc-ctl`. Their shared implementation is a private package under:

```text
tools/opencode_experiment/
  __init__.py
  cli_run.py
  cli_ctl.py
  config.py
  state.py
  client.py
  lifecycle.py
  observe.py
  feedback.py
  archive.py
  query.py
  tests/
```

The entry points require the caller's current directory to be inside one Git
worktree and resolve that worktree's root. Repository-local entry points must
also belong to that root. They reject a mismatch instead of silently
controlling another checkout; all `target/exp/` paths are therefore relative
to the repository selected by the caller.

The Python implementation may import only the standard library. Expected
facilities include `argparse`, `dataclasses`, `datetime`, `enum`, `fcntl`,
`hashlib`, `http.client`, `json`, `os`, `pathlib`, `shutil`, `subprocess`,
`tempfile`, `time`, `typing`, `unittest`, and `urllib`.

External executables are capability dependencies, not Python packages:

- `opencode` is required by `oc-run` and live control;
- `git` records repository identity;
- a plan may require a built artifact such as `target/debug/telora`;
- `jaq`, `jq`, or mise is required only by `query`; and
- plan validation commands may require their declared tools.

The control plane is plan-neutral. It contains no A1/A2 role names, ontology
paths, Telora commands, or plan-specific file roots. Each plan manifest owns
workspace layout, observed paths, permissions, artifacts, feedback location,
validation commands, and archive selection. The plan README owns the concrete
sequence in which Main combines `oc-run` and `oc-ctl` for that experiment.

Every external CLI is resolved uniformly through executable probes. For each
logical capability and its ordered CLI candidates, the controller first tries:

```text
<cli> --version
```

across all candidates. It then tries failed candidates through:

```text
mise x -- <cli> --version
```

The first successful probe is cached as the capability's argument prefix, for
example `{"curl": ["curl"], "query": ["mise", "x", "--", "jaq"]}`.
Subsequent call sites append arguments to that prefix. A plan declares commands
and capabilities, but does not describe mise tool names or detection mechanics.

## Identifiers and immutable binding

`plan-id` and `exec-name` match:

```text
[a-z0-9][a-z0-9._-]*
```

They contain no slash, whitespace, leading dot, `..` path component, or shell
interpretation. Resolved paths must remain beneath the repository's
`experiments/` and `target/exp/` roots respectively.

The first successful:

```text
oc-run <plan-id> <exec-name>
```

atomically creates:

```text
target/exp/<exec-name>/plan
```

whose complete contents are:

```text
<plan-id>\n
```

The file is immutable after creation. Reusing an existing `exec-name` is valid
only when its `plan` content equals the supplied `plan-id`. A different plan is
an identity error, never a rebind operation. All `oc-ctl` commands receive only
`exec-name`; they resolve and cross-check the plan through this file and
`state.json`.

## Plan manifest

Every migrated plan contains:

```text
experiments/<plan-id>/experiment.json
```

The initial schema is `telora.opencode-experiment/v1`. Unknown keys are
rejected so a misspelled permission or path cannot silently weaken the plan.
The manifest defines at least:

```json
{
  "schema": "telora.opencode-experiment/v1",
  "prompts": {
    "start": "Carry out the task.",
    "continue": "Continue.",
    "feedback": "Read the feedback file and assess it."
  },
  "workspace": {
    "template": "opencode-workspace",
    "copies": [
      {"from": "TASK-A2.md", "to": "a1/TASK-A2.md", "mode": "0444"}
    ]
  },
  "permissions": {
    "read": ["a1/**", "a2/**"],
    "write": ["a2/src/**", "a2/bin-src/**"],
    "commands": ["./bin/run", "./bin/run-test", "./bin/types", "./bin/show"]
  },
  "feedback": {
    "path": "a2/feedback.md",
    "role_writable": false
  },
  "validation": [
    {"name": "run", "command": ["./bin/run"]},
    {"name": "run-test", "command": ["./bin/run-test"]}
  ],
  "observe": ["a2"],
  "archive": ["a1", "a2", "opencode.json"]
}
```

`observe` selects the plan-owned workspace paths exposed by `files`,
`snapshot`, and the normalized query document; the controller does not assume
an output directory name. Paths are workspace-relative POSIX paths. Absolute paths, parent traversal,
symlink escape, duplicate destinations, and writes outside declared mutable
roots are rejected. Commands are argument arrays, never shell strings.

The manifest may declare an artifact source such as the repository's current
Telora binary and its workspace destination. Preparation builds the artifact
only through an explicitly declared command. Its digest is recorded after the
build and copy.

The plan owns exact nonempty `start`, `continue`, and `feedback` prompt strings.
`oc-ctl` sends their UTF-8 values without interpolation, prefix, or suffix.
The manifest digest makes these protocol inputs immutable for one execution.
General `ask` text is the only intentionally dynamic prompt class.

## Execution filesystem

Live and frozen state use:

```text
target/exp/<exec-name>/
  plan
  lock
  state.json
  handshake.log
  rounds/
    000-initial.json
    001-ask.json
    002-feedback.json
  feedback/
    001.md
    001.json
  result/
    session.json
    query.json
    workspace/
    validation/
    RUNLOG.md
    SUMMARY.md
```

The temporary workspace is not placed below `target/exp/<exec-name>/`. Its
absolute path is recorded in `state.json`; result collection copies only
declared paths into `result/workspace/`. The opencode session state remains in
opencode's own storage and is exported before shutdown.

`state.json` includes:

```json
{
  "schema": "telora.opencode-execution/v1",
  "plan_id": "ontology-edsl",
  "exec_name": "a2-017",
  "run_id": "...",
  "phase": "ready",
  "workspace": "/tmp/oc-exp-.../ws",
  "session_id": "ses_...",
  "server_url": "http://127.0.0.1:4096",
  "repository_revision": "...",
  "repository_dirty": false,
  "input_hashes": {},
  "binary_hashes": {},
  "next_round": 0,
  "active_round": null,
  "created_at": "...",
  "started_at": null,
  "finished_at": null
}
```

The separate `plan` file is authoritative for fast identity resolution;
`state.json.plan_id` is redundant by design and must match it.

## Locking and atomicity

Every state-changing command obtains an exclusive `fcntl.flock` on
`target/exp/<exec-name>/lock`. Read commands obtain a shared lock while reading
local state and release it before a potentially long event stream.

State updates use one implementation:

1. serialize canonical UTF-8 JSON into a sibling temporary file;
2. flush and `os.fsync()` the file;
3. replace the destination with `os.replace()`;
4. fsync the containing directory; and
5. release the lock.

For a remote mutation, the controller records intent, performs the API call,
then records success. If it crashes between remote success and local success,
the next invocation reconciles from session messages and message IDs rather
than sending a duplicate prompt. Idempotency is established from observed
remote state, not only local booleans.

## State machine

The execution phases are:

```text
absent -> preparing -> ready -> active <-> idle -> finishing -> finished
                                                        |
                                                        -> failed
finished -> retired
```

`active` and `idle` describe the controller's conversation lifecycle; the live
daemon's own `busy`/`idle` status is observed independently. An assistant
`finish=stop` closes only the active round and moves the execution to `idle`.
An assistant `finish=length` also leaves the server idle, but the round remains
recoverable through the constrained `continue` operation.

`failed` records an infrastructure or validation failure without deleting
evidence. A failed execution cannot be silently reset to `ready`; a new
`exec-name` is required unless the recorded failure is explicitly classified
as recoverable by this RFC.

## `oc-run`

```text
oc-run <plan-id> <exec-name> [--port PORT] [--artifact NAME=PATH]
```

On first execution, `oc-run`:

1. validates identifiers, repository, manifest, dependencies, paths, and port;
2. creates the execution root and immutable `plan` binding;
3. builds or resolves declared artifacts;
4. creates a temporary workspace and installs exact plan inputs;
5. generates the opencode permission configuration from the manifest;
6. briefly starts a local daemon if needed to create one empty,
   workspace-bound session;
7. records state, hashes, versions, and exact TUI command;
8. stops the temporary daemon; and
9. starts opencode's ordinary daemon-backed TUI for that session with inherited
   stdin, stdout, stderr, and terminal control, then waits for it to exit.

On resume, it cross-checks plan, workspace, session, hashes, and port. It never
creates a second session for an existing execution or refreshes plan inputs in
place. Changed plan inputs require a new `exec-name`.

`oc-run` does not send a hello, the initial prompt, feedback, or any model
message. The prepared session is empty.

The external `oc-run` process owns the TUI and daemon lifecycle. It does not
replace itself: retaining the small parent process is required for post-TUI
state reconciliation. `oc-ctl` never kills that process. After `oc-ctl finish`
has frozen the execution, the user exits the TUI. When the TUI child returns,
`oc-run` observes the finished phase and may remove the exact recorded
temporary run root after verifying that the frozen workspace copy and session
export exist. If the TUI exits before finish, `oc-run` retains the workspace
and execution identity for later resume and returns the TUI exit status.

## `oc-ctl` command surface

The initial command set is:

```text
oc-ctl doctor
oc-ctl workspace <exec-name>
oc-ctl start <exec-name>
oc-ctl status <exec-name>
oc-ctl snapshot <exec-name>
oc-ctl recent <exec-name> [N]
oc-ctl timeline <exec-name> [N]
oc-ctl events <exec-name>
oc-ctl files <exec-name>
oc-ctl failures <exec-name>
oc-ctl audit <exec-name>
oc-ctl answer <exec-name> [--json]
oc-ctl ask <exec-name> MESSAGE
oc-ctl ask <exec-name> --file PATH
oc-ctl continue <exec-name>
oc-ctl feedback <exec-name>
oc-ctl feedback-status <exec-name>
oc-ctl validate <exec-name>
oc-ctl export <exec-name>
oc-ctl finish <exec-name>
oc-ctl query <exec-name> QUERY
oc-ctl query <exec-name> --file PATH [--raw-output]
oc-ctl retire <exec-name>
```

All commands resolve the plan from `target/exp/<exec-name>/plan`. Commands that
need a live daemon fail with a specific unavailable status and do not mutate
state. Offline commands continue to work after TUI exit.

### Start

`start` requires `ready`, a healthy matching daemon, an empty session, and
unchanged initial-prompt and input hashes. It sends the exact initial prompt
once through the asynchronous prompt endpoint and records round 000. Repeated
invocation reconciles and reports the existing start rather than sending it
again.

Workspace preparation belongs to `oc-run`; `start` performs the final
preflight and begins model work. This preserves the external process boundary.

### Observation

`status`, `snapshot`, `recent`, `timeline`, `events`, `files`, `failures`, and
`audit` subsume the current ontology-specific observer. Their JSON output has a
stable schema; human-readable output is a rendering of the same document.

`events` streams filtered lifecycle events and suppresses token deltas by
default. It terminates cleanly on EOF, interrupt, or daemon loss and never
changes the session.

`answer` returns the final textual content and identity of the newest completed
assistant round. It does not concatenate intermediate tool-call messages. Its
JSON form includes execution, round kind, message ID, completion time, finish
reason, and text.

### Ask

`ask` begins an ordinary follow-up round. It requires an execution that has
started, is not finished, has no active round, has a live idle daemon, and whose
latest assistant message completed with `finish=stop`. The message is accepted
as one argument or read as exact UTF-8 from `--file`. It is saved with
`kind=ask`, its digest, and its resulting message identity.

`ask` is for Main's ordinary clarification and coordination. It is not treated
as formal downstream feedback.

### Continue

`continue` is a constrained recovery operation. It requires a live idle session
whose latest assistant message ended with `finish=length`. It sends exactly
the plan's `prompts.continue` value

once for that terminal message. It rejects `finish=stop`, busy sessions,
additional text, and duplicate recovery attempts. An automatic model-side
continuation observed before invocation is reconciled and is not duplicated.

### Feedback

`feedback` supports Main's star-topology relay among AI-2, AI-3, and AI-4. Main
reads a downstream answer, decides what evidence is relevant, and writes the
target execution's plan-declared feedback file, initially `a2/feedback.md` for
the ontology plan. The controller does not synthesize or semantically edit its
contents.

The command requires the same idle boundary as `ask`. It then:

1. validates the feedback path and rejects an empty file;
2. rejects a digest already delivered to this execution;
3. snapshots the exact file under `feedback/NNN.md`;
4. records round number, target, digest, time, and source provenance;
5. sends the plan's fixed feedback notification; and
6. records the remote user message ID after reconciliation.

Provenance may be supplied through a small sidecar written by Main or explicit
options such as source execution, source round, and source message ID. It
identifies downstream observations without claiming Main originated them.

The feedback file is Host-owned. It is readable by the role but denied by the
role's write/edit policy. A2, A3, or A4 may assess, reject, or act on the
feedback; the controller records the answer without deciding correctness.

Iterative feedback does not require an execution restart. Main keeps all
participating TUI processes alive, uses read-only live queries while a role is
`active`, obtains the completed answer after the role returns to `idle`, and
relays selected observations through a new `feedback` round on the target
execution. `finish=stop` terminates only that round. Main repeats this loop
until the experiment-level stopping condition holds, then invokes `finish` once
per execution.

The plan's fixed notification is neutral and directs the role to read the file,
assess each observation, update justified deliverables and validations, and
report the result. Feedback contents are not duplicated into the prompt.

### Validate

`validate` executes manifest-declared Host commands directly as argument
arrays from the prepared workspace. It captures command, cwd, start/end,
stdout, stderr, and exit status under the execution result area. Validation is
repeatable and does not send a model message. A plan distinguishes visible role
commands from Host-only validation commands.

### Export

`export` resolves the session from the execution name and writes the complete
raw opencode export atomically to
`target/exp/<exec-name>/session-export.json`. It reports the path, byte count,
and message count; callers do not invoke `opencode export` with a session ID.
The implementation connects the exporter to a temporary regular file before
parsing because large tool outputs are not reliably preserved through every
CLI pipe implementation. Malformed or failed exports are retried a bounded
number of times and never replace a valid prior export.

### Finish

`finish` requires no active round, a live idle daemon, and a latest assistant
message completed with `finish=stop`. It is the only operation that makes an
execution terminal. It:

1. reconciles all rounds and completion timestamps;
2. exports raw session data while the daemon is live;
3. collects normalized messages, events, status, statistics, and failures;
4. runs or verifies required Host validation;
5. copies manifest-declared workspace paths without following escaping links;
6. writes `result/session.json`, `result/query.json`, validation artifacts,
   hashes, `RUNLOG.md`, and a factual summary skeleton;
7. fsyncs the result tree and marks the execution `finished`; and
8. tells the user that the TUI may exit.

`finish` never terminates the TUI or daemon directly. The external `oc-run`
process performs exact temporary-directory cleanup after the TUI exits and
only when frozen evidence has passed completeness checks.
Workspace archiving is staged and atomically replaced so read-only inputs and
interrupted attempts remain safely retryable. Infrastructure errors restore
the pre-finish idle phase; required validation failures enter `failed`.

### Retire

`retire` is valid only for `finished` executions with complete frozen evidence.
It removes an exact recorded temporary workspace if still present and changes
the phase to `retired`. It does not delete `result/`, `plan`, state, rounds, or
feedback. It refuses broad, unresolved, symlinked, or non-temporary paths.

## Live and offline query document

Live collection and `finish` produce one normalized schema:

```json
{
  "meta": {},
  "state": {},
  "status": {},
  "rounds": [],
  "messages": [],
  "events": [],
  "summary": {},
  "failures": [],
  "files": [],
  "validation": []
}
```

While the daemon is live, `query` assembles this document from local state and
the API. After finish or daemon shutdown, it reads
`result/query.json`. The same query therefore has the same meaning before and
after shutdown. Raw API export remains separately available for audit.

Internal controller decisions use Python's `json` module and typed validation,
not jq expressions.

## Query backend selection

At least one of the following query backends must be available when `query` is
used:

```text
jaq
jq
mise x -- jaq
mise x -- jq
```

Automatic selection order is:

1. successful `jaq --version`;
2. successful `jq --version`;
3. successful `mise x -- jaq --version`; and
4. successful `mise x -- jq --version`.

`OC_QUERY_ENGINE` may select `jaq`, `jq`, `mise-jaq`, or `mise-jq`. An explicit
selection is strict and does not fall back. Both direct and mise backends use
the common `--version` probe. Because a mise probe may install a tool,
`doctor` exposes the selected command prefix and `query` reports the selected
mise backend on stderr.

The selected command is represented as an argument prefix:

```text
["jaq"]
["jq"]
["mise", "x", "--", "jaq"]
["mise", "x", "--", "jq"]
```

`query` appends the user expression or `-f` file and optional `-r`, passes the
normalized JSON document on stdin, and preserves stdout, stderr, and exit code.
It does not expose arbitrary backend argument passthrough. jaq/jq compatibility
is the query author's responsibility.

The backend kind and version may be recorded as query-environment metadata but
are not experiment inputs and do not affect input hashes or comparability.
Missing query tools do not block `oc-run`, `start`, observation, validation, or
finish.

## Doctor

`oc-ctl doctor` reports without changing experiment state:

- Python version and standard-library compatibility;
- repository root and Git revision/dirty status;
- opencode path and version;
- direct jaq/jq availability;
- mise availability and selectable tool commands;
- current query backend and override source; and
- plan-specific artifact/tool readiness when a plan or execution is supplied.

The implementation includes a test that imports every Python module in an
environment containing only the standard library. Import of `requests`,
`click`, `pydantic`, `httpx`, or any other third-party package is forbidden.

## Multi-agent orchestration

The control plane supports the RFC 0217 star topology without embedding that
experiment's semantic workflow. Main may control executions such as:

```text
oc-run ontology-a2 a2
oc-run enterprise-a3 a3
oc-run intent-a4 a4
```

in separate external terminals, then use:

```text
oc-ctl start a2
oc-ctl answer a2 --json
oc-ctl start a3
oc-ctl answer a3 --json
oc-ctl feedback a2
oc-ctl ask a3 --file clarification.md
oc-ctl finish a4
```

Main owns semantic relay. Infrastructure records which completed answer became
the source of a feedback artifact, but does not automatically copy an answer
into another role's context. Future artifact publication/import commands may
automate frozen upstream delivery; they are deferred until the first A2-A4 plan
specifies exact ownership and mapping rules.

## Ontology eDSL migration

Implementation of this RFC is incomplete until the current
`experiments/ontology-edsl/` workflow runs entirely through the new control
plane.

The migration must:

1. add `experiments/ontology-edsl/experiment.json`;
2. add the fixed manifest prompts and a Host-owned feedback seed/path;
3. express all current injected files, workspace templates, Telora artifact,
   permissions, wrappers, validation, and archive paths in the manifest;
4. update `experiments/ontology-edsl/README.md` to use only `oc-run` and
   `oc-ctl` for new runs;
5. reproduce preparation, attach/resume, empty-session start, snapshot, status,
   recent, timeline, audit, continue, files, events, finalization, validation,
   and archive behavior;
6. run a fresh ontology execution with a named `exec-name` and verify the exact
   input/binary hashes and four fixed commands;
7. verify TUI exit before finish remains resumable;
8. verify finish followed by TUI exit freezes and cleans the temporary
   workspace without losing results;
9. delete `open-opencode-tui.sh`, `control-opencode.sh`, and
   `observe-opencode.sh`; and
10. remove current-protocol instructions from the ontology README rather than
    keeping two normative workflows.

Historical `target/opencode-test-*` directories are read-only evidence. They
are neither moved nor rewritten. A new execution uses:

```text
target/exp/<exec-name>/
```

and its `result/` is the canonical archive. The currently active legacy
`target/exp/` shape is temporary controller state, not an execution archive.
During the one-time plan migration it is discarded, not converted or guessed
into a named execution. Historical experiment results are not migrated.

## Error behavior

CLI errors use stable categories and nonzero exit statuses for at least:

- usage and invalid identifiers;
- missing or invalid plan;
- execution/plan mismatch;
- corrupted or unsupported state schema;
- unavailable daemon;
- session/workspace identity mismatch;
- illegal phase transition;
- busy session;
- unexpected terminal finish reason;
- duplicate ask, feedback, or continue delivery;
- changed input digest;
- unsafe path or symlink escape;
- validation failure;
- incomplete finish/export; and
- unavailable query backend.

Human diagnostics include the exact execution and failed invariant. Machine
consumers may request JSON error output. Tracebacks are suppressed for expected
errors and retained only behind an explicit debug option.

## Security and isolation boundary

This remains soft experiment isolation, not an adversarial sandbox. The control
plane nevertheless enforces:

- resolved paths beneath declared roots;
- immutable plan inputs and feedback snapshots;
- role write denial for Host-owned feedback;
- exact command arrays and working directories;
- no shell evaluation of plan commands or messages;
- loopback-only daemon addresses;
- session/workspace identity checks on every live operation;
- exact temporary cleanup targets; and
- no recursive deletion based on unresolved variables, broad roots, or globs.

Credentials and opencode provider configuration remain external. They are not
copied into result archives.

## Testing strategy

Tests use `unittest`, `tempfile`, and `http.server.ThreadingHTTPServer`. They do
not require third-party Python packages or a live provider for unit coverage.

Required coverage includes:

- identifier and path traversal rejection;
- manifest validation and unknown keys;
- immutable exec-to-plan binding;
- atomic state replacement and lock exclusion;
- daemon API success, malformed JSON, HTTP failure, timeout, and reconciliation;
- empty-session preparation and start idempotency;
- ask, feedback, answer, and continue phase guards;
- duplicate feedback digest and provenance persistence;
- `finish=stop`, `finish=length`, busy, and automatic-continuation cases;
- live and offline normalized query equivalence;
- each direct and mise query backend command construction;
- query stdout/stderr/exit propagation;
- finish interruption and restart recovery;
- archive path and symlink safety;
- TUI exit before/after finish behavior;
- no third-party imports; and
- ontology plan migration end to end with a fake API plus one real smoke run.

## Implementation plan

1. implement standard-library config, identifier, lock, atomic JSON, and state
   modules with unit tests;
2. implement the opencode HTTP client and fake-server contract tests;
3. implement `oc-run` preparation, handshake, resume, inherited-terminal TUI
   supervision, and cleanup;
4. implement start and read-only observation commands;
5. implement answer, ask, continue, and formal feedback rounds;
6. implement validation, finish, archive, retirement, and normalized queries;
7. implement jaq/jq/mise backend discovery and doctor;
8. add the ontology eDSL manifest prompts;
9. migrate its README and run a fresh named execution;
10. remove the three legacy ontology controller scripts; and
11. record the accepted command/state schemas as the infrastructure SSOT.

The RFC is committed before implementation begins. Infrastructure changes may
be split into reviewable commits, but the legacy scripts remain until their
replacement passes the migration acceptance criteria.

## Rejected alternatives

### Continue with Shell and jq

Shell remains suitable for fixed workspace wrappers such as `./bin/run`, but
not for the control plane's JSON state machine, HTTP reconciliation, atomic
updates, multi-session locking, and feedback provenance. Long quoted commands
also cause avoidable approval and observability friction.

### Node.js

Node.js offers convenient asynchronous HTTP, but adds a package/toolchain
surface without solving the primary filesystem, flock, atomic replacement,
and process-exec concerns more directly than Python. The expected concurrency
does not require an async framework.

### Third-party Python libraries

`requests`, `httpx`, `click`, `pydantic`, and similar packages would require an
installation environment and dependency lock for functionality covered by the
standard library. They are forbidden for this tool.

### Put execution identity inside the plan directory

This prevents clean repeated runs and concurrent stages. Plans are immutable
definitions; executions are named state and evidence.

### Make `start` launch the daemon

That gives Main ownership of the external process and reintroduces repeated
permission/lifecycle problems. `oc-run` prepares and owns the TUI/daemon;
`oc-ctl start` performs final preflight and submits work.

### Treat every `finish=stop` as execution completion

This prevents ordinary questions and A2-A4 feedback correction rounds. A stop
ends a round; explicit `finish` ends the execution.

### Automatically relay downstream answers

Downstream answers contain claims, guesses, summaries, and possibly irrelevant
content. Main must select reproducible observations and preserve their source.
The controller reliably transports and records feedback but does not author it.

### Flatten same-shape records or messages during collection

Raw session export remains authoritative. Normalization adds indexes and
statistics without replacing raw message structure or losing tool-call data.

### Kill the TUI from `finish`

This can look like an accidental session abort and races with final display.
`finish` freezes evidence; the human exits the TUI; `oc-run` then performs
verified cleanup.

## Acceptance criteria

RFC 0224 is implemented when all of the following hold:

1. `oc-run <plan-id> <exec-name>` creates or resumes exactly one external TUI
   session and writes the immutable plan binding;
2. `oc-ctl` resolves every operation from `exec-name` alone;
3. multiple named executions coexist without shared mutable identity;
4. state transitions are locked, atomic, validated, and restart-recoverable;
5. start, observation, answer, ask, continue, feedback, validation, finish,
   query, and retire obey their specified guards and idempotency;
6. a stop ends a round while explicit finish ends the execution;
7. feedback snapshots preserve digest, source provenance, prompt identity, and
   completion identity while remaining role-read-only;
8. live and frozen query documents support the same jaq/jq expression;
9. direct jaq/jq and both mise command forms are discovered and selectable;
10. all Python code imports and runs with only Python 3.11 standard-library
    modules;
11. unit and fake-server integration tests cover failure and recovery paths;
12. the ontology eDSL plan completes a fresh real named execution with all four
    fixed commands successful;
13. its README contains only the new workflow for new executions;
14. its three legacy control scripts are deleted after migration validation;
15. historical experiment archives remain byte-for-byte unchanged; and
16. a finished execution remains fully queryable after its TUI and daemon exit.
