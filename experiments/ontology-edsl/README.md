# Ontology eDSL experiment

This directory is the stable source anchor for the ontology eDSL creation
experiment. It separates versioned experiment inputs from per-run artifacts.

## Files

- `A2-PROMPT.md` is the minimal exact initial prompt passed to the eDSL
  implementer.
- `TASK-A2.md` defines the A2 role, workspace boundary, task, and completion
  requirements.
- `TELORA-TUTORIAL.md` is the bounded language tutorial supplied to the eDSL
  implementer.
- `TELORA-CLI.md` is the fixed self-validation workflow supplied to the eDSL
  implementer in self-validating runs.
- `EDSL-DESIGN.md` is the normative, domain-neutral behavior contract supplied
  to the eDSL implementer.
- `EVAL-METHOD.md` defines the Host-side process and outcome evaluation.
- `README.md` defines how an experiment run is prepared and archived.
- `opencode-workspace/` contains the versioned workspace files and validation
  wrappers used by self-validating runs.
- `open-opencode-tui.sh` creates or resumes the isolated self-validating
  opencode environment and enters its TUI.
- `control-opencode.sh` starts the prepared A2 task and records its successful
  completion through a stable, stateful interface.
- `observe-opencode.sh` provides a stable, read-only interface for Host-side
  status, recent-message, file, and filtered-event observation.

`TASK-A2.md`, `TELORA-TUTORIAL.md`, `TELORA-CLI.md`, and `EDSL-DESIGN.md` are
injected into a self-validating run's `a1/` directory. The controller sends the
exact UTF-8 contents of `A2-PROMPT.md` as the initial user prompt, without a
prefix, suffix, or run-specific interpolation. `EVAL-METHOD.md` remains
Host-only: exposing its hidden acceptance fixtures or evaluation guidance to
A2 would change the experiment.

## Experiment records

Completed runs are frozen under a numbered directory in `target/`. Their exact
layout depends on the protocol and input revision; a Host-relayed run commonly
has this shape:

```text
target/opencode-test-N/
  a1/
    TASK-A2.md
    TELORA-TUTORIAL.md
    TELORA-CLI.md
    EDSL-DESIGN.md
  a2/
    ontology-edsl/
    EDSL_TUTORIAL.md
    AI3_CONTRACT.md
    STAGE2_NOTES.md
  host-validation/
  RUNLOG.md
  SUMMARY.md
```

The archive is evidence, not live controller state. `a1/` contains the exact
inputs exposed to A2, `a2/` contains A2's output, `host-validation/` contains
Host-only checks, and `RUNLOG.md` records the input hashes, Telora revision,
model identity, runner configuration, protocol, and evaluation-method revision.
`SUMMARY.md` reports the result under that recorded protocol.

The initial prompt only directs A2 to the stable task. `TASK-A2.md` owns the
role, filesystem boundary, and completion instruction; the tutorial owns
language facts; the CLI document owns the validation interface; and the design
document owns observable eDSL behavior.

Historical `target/opencode-test-*` inputs and outputs are immutable. Changes to
this anchor apply only to later runs. When the stable input changes, the next
run identifies it as a new input revision rather than claiming verbatim
comparability with an older run.

## Experimental boundary

Every run keeps repository ontology examples, earlier experiment outputs, Host
fixtures, and this Host-only evaluation method outside A2's visible boundary.

The original Host-relayed protocol gives A2 read access to `a1/` and write
access to `a2/`. A2 cannot run Telora or Cargo; the Host executes checks and
relays observations. Historical runs using this protocol remain evidence of
that protocol and input revision.

The Host may report:

- the command category that failed;
- source diagnostics with locations;
- the hidden scenario name or behavior under test;
- expected and actual observable values; and
- whether a failure is static, runtime, diagnostic, or protocol-level.

The Host must not provide a reference implementation, algorithm name,
pseudocode, or a patch. A2 remains responsible for design and correction.

## Self-validating opencode runs

The self-validating protocol gives A2 read access to `a1/` and `a2/`, and write
access only to `a2/src/` and `a2/bin-src/`. A2 may execute these fixed commands:

```text
./bin/run
./bin/run-test
./bin/types
./bin/show
```

`./bin/run-test` always evaluates the single scratch entry
`a2/bin-src/test.telora`. The permission configuration contains one exact rule
for each command, so additional arguments and compound shell commands remain
denied without exposing a numbered command space. The wrappers pin every
source path and the copied Telora executable. `a2/telora-deps.json` is readable
and immutable; its empty dependency map prevents path dependencies from
crossing the workspace boundary.

### Prepare or attach

Prepare the first session, or attach to the recorded active session, with:

```text
experiments/ontology-edsl/open-opencode-tui.sh
```

The launcher accepts `--port` and `--telora`. It defaults to loopback port
`4096`, uses `target/debug/telora` or builds it when necessary, and rejects a
port occupied by another service. With no active identity under `target/exp/`,
it creates a fresh workspace under `/tmp`, briefly starts a headless daemon,
creates an empty workspace-bound session through the local API, and stops that
daemon. It then replaces itself with the ordinary opencode TUI, passing the
recorded session ID and fixed port. It does not send `A2-PROMPT.md` or invoke a
model.

When `target/exp/` already records a workspace and session, the launcher
resumes that identity. It never silently creates a second run or replaces an
existing session. The daemon and TUI have the same lifecycle: the daemon is
available while the TUI is open and stops when the TUI exits.

The generated layout is:

```text
/tmp/test-XXXXXX/
  ws/
    a1/
      TASK-A2.md
      TELORA-TUTORIAL.md
      TELORA-CLI.md
      EDSL-DESIGN.md
    a2/
      telora-deps.json
      src/
      bin-src/
        main.telora
        test.telora
    bin/
      telora
      run
      run-test
      types
      show
    opencode.json

target/exp/
  lock
  dir
  session-id
  server-url
  HANDSHAKE.log
  SESSION.json
```

The files under `target/exp/` are active controller state and remain outside
`ws/`. `dir` and `session-id` are the stable run identity. `server-url` records
the fixed TUI endpoint, while `HANDSHAKE.log` records the temporary
session-creation server. `SESSION.json` captures the repository state, input
hashes, exact TUI command, and task start and completion state. While the TUI is
running, its `/event` endpoint exposes the live process stream to an external
observer.

### Start

Once the TUI is ready, the Host confirms the empty session through the observer
and starts A2 with:

```text
experiments/ontology-edsl/control-opencode.sh start
```

The controller verifies the recorded prompt hash and empty session before it
sends the exact UTF-8 contents of `A2-PROMPT.md`. It records `task_started` and
`task_started_at` only after the asynchronous prompt request succeeds and
refuses to submit the task twice. Session preparation and experiment start are
separate events; ordinary setup does not add a synthetic `hello` turn to the
experiment context.

### Observe

The Host observes an active run through one fixed command surface:

```text
experiments/ontology-edsl/observe-opencode.sh snapshot
experiments/ontology-edsl/observe-opencode.sh status
experiments/ontology-edsl/observe-opencode.sh recent 3
experiments/ontology-edsl/observe-opencode.sh files
experiments/ontology-edsl/observe-opencode.sh events
```

`snapshot` combines health, session status, the three newest assistant steps,
and the current `a2/` file list. `events` filters token deltas and retains only
session status, completed messages, completed or failed tools, and errors. The
observer never sends a prompt or abort request.

### Finalize and exit

After observation shows an idle session whose final assistant message completed
with `finish=stop`, record the server-reported completion time with:

```text
experiments/ontology-edsl/control-opencode.sh finalize
```

Run `finalize` while the TUI is still open: the controller must query the live
daemon to verify the idle session and final assistant message. On success it
records `task_completed` and `task_completed_at` in `SESSION.json`. The TUI may
then exit. Both controller operations are idempotent. The controller does not
start, stop, or abort the daemon; the TUI owns that lifecycle.

### Archive and start another run

The current scripts manage one active identity and do not archive it or reset
`target/exp/`. After finalization, the Host freezes the run under a new
`target/opencode-test-N/` directory. The archive retains the injected `a1/`
inputs, A2's `a2/` output, `SESSION.json`, Host validation artifacts,
`RUNLOG.md`, and `SUMMARY.md`.

Only after that archive is complete does the Host retire `target/exp/`. Its
removal is the explicit boundary between runs: the next invocation of
`open-opencode-tui.sh` then creates a new temporary workspace and empty
session. Leaving `target/exp/` in place always resumes the same run.

Self-validating runs and Host-relayed runs measure different developer
conditions. Report their protocol and input revision explicitly rather than
treating their iteration counts as directly comparable.

## Interpretation

The experiment evaluates whether an isolated model can create a reusable,
typed ontology eDSL from stable language and behavior specifications. A
self-validating run additionally evaluates whether the bounded Telora feedback
loop is sufficient for independent correction. Neither protocol measures
memorization of Telora syntax, and one successful run does not establish
generalization to other domains.

Results are reported separately for language learnability, eDSL contract
compliance, enterprise extensibility, diagnostic quality, convergence, and
boundary preservation. They are never collapsed into a single pass rate.
