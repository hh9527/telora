# Four-AI Ontology Experiment Runbook

This runbook is normative for the `soft-reproducible-v1` execution profile. A controller stages
versioned files; it does not compose new role instructions during a run.

## Fixed Environment

All roles inherit the same installed Codex binary and existing Codex configuration. Do not create
a per-run `CODEX_HOME`, override the model/provider/base URL, inject a different credential source,
or select a different network path for a role. Record the effective Codex version, model, provider,
and configuration fingerprint once during preflight.

The only role runners are a pre-registered set of three persistent built-in sub-agents created with
`spawn_agent` and no inherited conversation history. Their canonical names are the durable A2, A3,
and A4 identities. Main communicates with them using collaboration messages and is the only
feedback relay. External, CLI, service-based, or newly substituted role runners are forbidden,
including as fallbacks. Agent creation happens only in method preflight, before a formal run id
exists.

## Agent Pool Preflight

Before creating a formal run, register the three identities in `agent-registry.template.yaml`.
For each identity:

1. prepare an empty absolute workspace;
2. call `spawn_agent` with no inherited turns and the fixed bootstrap boundary;
3. record the returned canonical agent name and workspace;
4. require the exact response `READY`;
5. confirm all three identities with `list_agents`.

Sequential registration avoids the collaboration slot limit. Persist the completed registry with
the execution-profile and bootstrap hashes. If any identity cannot be registered, no formal run
starts. Do not substitute an agent, reuse an agent from another method version, or switch runner.

Every formal run uses this fixed mapping. Each role retains its own correction history. AI-4 runs
the fixed trials sequentially, and Main records the trial boundary in every assignment.

The registry binds a method-bundle hash covering the controller, profile, runbook, preparer, role
and assignment files, AI-4 corpus, and stable intent tutorial. Any bundle change invalidates the
pool even when the bootstrap and execution profile themselves did not change.

Role-specific launch parameters are limited to:

1. the resolved working directory;
2. the read/write contract from `execution-profile.yaml`;
3. the command list from `execution-profile.yaml`; and
4. the matching versioned role file.

## Workspace Shape

Create an independent local Git repository for every role or correction round:

```text
workspace/
  bin/telora
  requirement/
    ROLE.md
    staged inputs...
  crates/
    frozen dependencies...
    owned crate...
```

Copy the matching `roles/*.md` file verbatim to `requirement/ROLE.md`. Copy
`intent-tutorial.md` verbatim for every AI-4 trial. Resolve dependency paths before launch. Do not
include repository history, RFCs, Host fixtures, another stage's notes, or unaccepted output.

Use the versioned preparer rather than assembling these directories manually:

```text
prepare-workspace.sh ai2 DEST TELORA
prepare-workspace.sh ai2-correction DEST TELORA PRIOR_CANDIDATE FEEDBACK
prepare-workspace.sh ai3 DEST TELORA ACCEPTED_EDSL
prepare-workspace.sh ai4 DEST PUBLIC_INTENT PUBLIC_API REQUEST
```

The preparer refuses an existing destination, copies only the fixed role inputs, initializes the
independent Git repository, commits the baseline, and prints its commit id. The controller records
that id in the run manifest. `PUBLIC_API` is a Host-reviewed interface-stub artifact derived from
the accepted enterprise public module; it must contain declarations only, never implementation
bodies.

An AI-2 correction uses `ai2-correction`. It stages the complete frozen prior candidate plus one
bounded `requirement/FEEDBACK.md`; it never reuses the prior mutable workspace. The same registered
AI-2 identity receives the correction so its role history is retained within one formal run.

## Preflight

Complete preflight before launching AI-2:

- repository revision and dirty status recorded;
- system Codex version/configuration fingerprint recorded;
- one provider request succeeds through the normal inherited environment;
- staged Telora binary runs a neutral check;
- every stable input exists and its hash is recorded;
- every role workspace can be assembled from the profile;
- Host validation dependency mappings resolve transitively;
- GitHub issue read/write succeeds;
- Issue body contains the fixed stage task list.

Failure means the run has not started. Fix the controller or environment, create a new run root,
and repeat preflight. Do not redesign permissions after role execution begins.

## Launch And Monitoring

A launch is committed only after the controller records:

```text
runner id or session id
resolved workspace path
observed running state
launch timestamp
```

From `stage-ready`, Main uses `followup_task` on the registered canonical identity with the fixed
assignment and resolved workspace. No `spawn_agent` is allowed during a formal run. Main observes
the identity with `list_agents` and receives its messages. Closing or escaping the human-to-Main
chat is not an instruction to stop a role and must never cause Main to call `interrupt_agent`.
Only an explicit human stop instruction authorizes interruption. File modification times are
progress hints, not proof that a runner is alive.

RPC responses use a one-minute controller timeout. Role turns use a fixed thirty-minute terminal
timeout and must not be terminated by the shorter RPC deadline. On a monitoring anomaly, use the
controller's read-only `inspect` operation before classifying the thread; never send a probe turn.
Bootstrap turns have a separate two-minute terminal deadline. Pool registration persists a partial
registry after every completed identity and records the failing identity and error; a partial or
failed registry is never eligible for a formal run.

The experiment must not start without two passing smoke tests and a complete registered pool.

Update the GitHub Issue in Chinese on material transitions and blockers. Keep current state and
the task list in the issue body; append event details as comments.

## Delivery And Validation

When a role reports completion:

1. stop treating its workspace as mutable;
2. verify changes are confined to the owned paths;
3. verify frozen input hashes are unchanged;
4. snapshot the candidate under the run's accepted-candidate area;
5. validate from a separate Host directory with explicit dependency mappings;
6. run executable checks and semantic review;
7. accept or write a bounded feedback file for a new correction workspace.

The Host never edits candidate source. A correction agent reads the same staged inputs, the prior
candidate, and one versioned feedback packet, and modifies only its owned output.

## Failure Classes

Classify failures before taking action:

- `runner-failure`: no model delivery because launch, provider, network, or session failed;
- `model-failure`: the model received the task but did not produce the required delivery;
- `candidate-failure`: a complete delivery failed executable or semantic Host validation;
- `protocol-defect`: the fixed profile omitted a required capability or input;
- `contamination`: a role used forbidden input or modified a frozen path.

Runner failures do not consume diagnostic correction rounds. A protocol defect ends the run; fix
the versioned plan and start a new run rather than changing permissions in place.

## Method Repair Gate

When progress is blocked by workspace construction, runner selection, permissions, command lists,
monitoring, dependency staging, or another part of the experiment method:

1. stop active role runners and leave their workspaces untouched;
2. classify the event as a controller or protocol defect;
3. do not send a new role instruction and do not switch runners;
4. patch the versioned profile, role files, preparer, corpus, or runbook as appropriate;
5. validate the method change with a bounded preflight or workspace smoke test;
6. mark the current run superseded, preserving its records;
7. start a new run id from the complete preflight.

Operational retry is allowed only when the fixed method was invoked correctly and an external
transient failure occurred. It must reuse the same built-in identity, workspace baseline,
permissions, commands, and role file. This gate ensures that every workaround which proves
necessary becomes part of the reusable method instead of disappearing into controller messages.

## Stage 4

Use `stage4-trials.yaml` as the fixed corpus. The same registered AI-4 identity executes the trials
sequentially, retaining its role history, while each trial receives an independent workspace.
Stage only one trial request as `requirement/REQUEST.md`; never stage the expected classification.
AI-4 does not execute Telora. The Host checks and runs expressible
intents against the accepted enterprise model. An unrepresentable request is classified from the
explicit refusal artifact and absence of invented identifiers.

## Completion

The final report must distinguish functional results, model correction rounds, runner recovery,
protocol deviations, and isolation grade. `soft-reproducible-v1` supports a reproducible staged
knowledge-transfer claim, not an adversarial filesystem-security claim.
