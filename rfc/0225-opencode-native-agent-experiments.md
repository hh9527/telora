# RFC 0225: Opencode Native-Agent Experiments

- Status: Proposed
- Depends on: RFC 0224

## Summary

Extend the single-session experiment controller from RFC 0224 with opencode's
native primary and subagent model. One coordinator session owns an experiment,
delegates work to permission-scoped subagents, and resumes those same child
sessions for feedback rounds. The controller observes and archives the native
session tree; it does not implement a second role or message-delivery system.

This RFC replaces RFC 0224's v1 manifest, Host-mediated feedback command, and
single-agent permission generation. It retains RFC 0224's named execution,
external TUI lifecycle, observation, query, validation, and archival model.

## Motivation

An A2-A3-A4 experiment needs independent visibility boundaries and repeated
feedback. Separate workspaces, TUI processes, delivery transactions, role
state machines, and custom recovery rules duplicate facilities already present
in opencode. They also hide the actual multi-agent behavior being evaluated.

Opencode already provides:

- named primary and subagent definitions;
- per-agent file, command, and task permissions;
- child sessions created by `task`;
- continuation through a returned task session ID; and
- HTTP endpoints for parent messages, children, child messages, and status.

## Design

Each plan is an independent Git repository mounted under `experiments/` as a
submodule. Its committed tree already has the final opencode workspace layout,
including `opencode.json`. `oc-run <plan-id> <exec-name>` requires a clean,
committed plan worktree, records its commit and origin, clones that commit into
one temporary shared workspace, injects Host artifacts, creates one empty
coordinator session, and opens one TUI. `oc-ctl` addresses the named execution,
never an invented role execution.

The cloned workspace is therefore a Git worktree before opencode starts.
Opencode evaluates read and edit paths relative to the worktree; cloning also
removes the need for Host template/copy assembly and makes the plan commit a
complete reproducibility identity.

The plan owns native `.opencode/agents/<name>.md` role files. Frontmatter holds
OpenCode metadata and permissions; the Markdown body is the role prompt.
`opencode.json` selects the default coordinator and does not duplicate role
definitions. The coordinator MUST NOT edit experiment artifacts or execute
Host commands. Its `task` permission admits only declared experimental roles.
Subagents MUST NOT delegate further unless a future plan explicitly requires
and documents it. OpenCode owns parsing and validation of these native role
files; the Host controller does not implement a second YAML or role validator.

The plan also contains `experiment.json` for Host prompts, artifact injection,
OpenCode daemon environment, validation, observation, and archival metadata.
Daemon environment is an explicit allowlist rather than an arbitrary process
environment map. The initial allowlist contains the positive integer
`OPENCODE_EXPERIMENTAL_OUTPUT_TOKEN_MAX`; it lets a plan raise OpenCode's
default 32000-token generation ceiling without allowing `PATH`, credentials,
or configuration locations to be replaced. `oc-run` injects the recorded value
into both the temporary handshake daemon and the TUI-owned daemon. The value is
snapshotted in execution state and cannot change when an execution resumes.

`experiment.json` remains in the cloned tree, while the committed role files
define whether agents can observe it. These permissions are closed when the
plan is authored. The controller neither parses nor security-audits OpenCode
roles; it drives sessions, observes and interprets state, and archives evidence.

Agents share a workspace so a downstream role can consume an upstream public
artifact directly. Sharing storage does not grant visibility: each role has an
explicit relative permission allowlist. Private design inputs, hidden domain
facts, implementation notes, source, and fixtures remain denied even when a
role can list the containing public directory.

The coordinator records the session ID returned by each native task. A feedback
round MUST resume that child ID instead of creating a replacement child. The
workspace artifacts are the feedback channel; the coordinator may announce
that feedback changed but MUST NOT rewrite, summarize, or enrich it.

The coordinator is not a task-definition layer. Its role definition is an
ordered transition protocol whose clauses have the form "when observable state
X holds, perform action Y". It advances the workflow only from child status,
public artifact presence, validation status, and recorded session IDs. A
running child causes no transition; an incomplete delivery can only resume the
recorded child, never create a replacement. Every child invocation uses a
fixed lifecycle-only prompt that directs the child to follow its own agent
prompt and visible files. The coordinator MUST NOT explain, restate, infer,
narrow, or extend domain semantics, implementation language, scope,
deliverables, or acceptance criteria in a task call.

Every role protocol specifies, for each accepted event: the exact instruction
or state change, the evidence used to decide, the action performed, and the
observable completion criteria. Host-to-coordinator and coordinator-to-child
instructions are finite exact-text allowlists. Text outside the relevant
allowlist causes no workflow mutation. A whitelisted instruction whose state
precondition is false also causes no transition.

## Control Surface

RFC 0224 commands remain single-execution commands:

```text
oc-run <plan-id> <exec-name>
oc-ctl start <exec-name>
oc-ctl status <exec-name>
oc-ctl recent <exec-name>
oc-ctl query <exec-name> <jq-expression>
oc-ctl finish <exec-name>
```

RFC 0225 adds read-only native-tree observation:

```text
oc-ctl children <exec-name>
oc-ctl tree <exec-name>
oc-ctl child-recent <exec-name> <session-id> [count]
oc-ctl child-continue <exec-name> <session-id>
```

There are no `deliver`, `feedback`, role-start, or role-finish commands. Native
tasks and shared, permission-scoped files are the workflow.

`child-continue` is recovery for an interrupted child stream. It sends only the
plan's fixed continuation prompt to an existing direct child; it cannot create
or substitute a role session.

## Observation And Recovery

The HTTP client retries short connection refusals. An opencode process may keep
its listener and continue model work while temporarily refusing requests under
high CPU load; one refused observation is not daemon death.

The controller observes:

- the coordinator status and messages;
- direct children of the coordinator;
- messages of each child; and
- artifacts in declared observe roots.

`finish` exports the coordinator and every direct child, saves child message
evidence, validates Host commands, and archives the shared workspace. A failed
observation does not abort the model session. `abort` is not part of the normal
control surface.

## Ontology Experiment

The first plan uses this workspace shape:

```text
docs/{LANG-TUTORIAL.md,TELORA-CLI.md}
bin/telora
.opencode/agents/{coordinator.md,a2.md,a3.md}
ontology/{GOAL.md,DESIGN.md,DSL-TUTORIAL.md,telora-deps.json,src/,bin-src/}
ent-1/{GOAL.md,DOMAIN.md,FEEDBACK.md,telora-deps.json,src/,bin-src/}
```

A2 owns `ontology/`; its task SSOT is private `ontology/GOAL.md`, while
`DESIGN.md` contains the behavioral design contract. It later reads only
`ent-1/FEEDBACK.md`. A3 owns `ent-1/`; its task SSOT is private
`ent-1/GOAL.md`, while `DOMAIN.md` contains enterprise facts. It also reads
public docs and the public ontology tutorial and contract. A3 cannot read the
ontology goal, design, source, validation entries, or notes. The coordinator
can see only public handoff and progress artifacts, not either role's goal.

The ontology contract treats the reusable eDSL as a modelling factory rather
than only a validation framework. A2 owns the domain-neutral factory,
canonical relational Plan IR, automatic plan assembly, coverage guarantees,
and deterministic Plan-to-SQL transform. A3 supplies structured enterprise
knowledge and obtains a Model from the public factory. A query intent compiled
through that Model must expose the intermediate Plan and produce the same SQL
as the public composed query creator; A3 cannot provide an opaque final-plan
builder or bypass the Plan by concatenating SQL.

Because Telora is not assumed prior knowledge, A2 and A3 role definitions tell
agents to learn from the public language and CLI tutorials and permit small
exploration entries under their own `bin-src/` roots. Those probes may run via
`telora run`, `types`, and `show`; they do not expand either role's read scope.

The expected native sequence is:

```text
A2 create -> A3 model and feedback -> resume A2 -> resume A3
```

Completion evidence includes exactly the intended child identities and proves
that revision used the original A2 and A3 sessions.

## Rejected Design

A custom workflow controller with one workspace and TUI per role, explicit
deliver/feedback transactions, digest-based revisions, and a parallel role
lifecycle is rejected. It adds an orchestration product beside opencode and
makes the experiment test that product rather than the agents and language.
