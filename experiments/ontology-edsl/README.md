# Ontology eDSL experiment

This directory is the versioned `ontology-edsl` experiment plan. Named
executions use the repository-wide opencode control plane defined by
RFC 0224; transient state and frozen results live under
`target/exp/<exec-name>/`.

## Plan inputs

- `TASK-A2.md` defines the A2 role, workspace boundary, task, and completion
  requirements.
- `TELORA-TUTORIAL.md` is the bounded language tutorial supplied to A2.
- `TELORA-CLI.md` defines A2's fixed self-validation commands.
- `EDSL-DESIGN.md` is the normative, domain-neutral behavior contract.
- `experiment.json` contains the exact start, continuation, and feedback
  prompts in its `prompts` object.
- `EVAL-METHOD.md` is Host-only evaluation guidance and is never injected.
- `experiment.json` declares injected files, permissions, artifact,
  validation, feedback, and archive policy.
- `opencode-workspace/` contains the initial `a2/` tree and fixed wrappers.

The four tutorial and task documents are copied read-only into `a1/`. A2 may
read `a1/` and `a2/`, edit `a2/src/`, `a2/bin-src/`, and the three required
root-level deliverable documents, and execute only:

```text
./bin/run
./bin/run-test
./bin/types
./bin/show
```

`a2/feedback.md` is readable but Host-owned. The controller freezes each new
nonempty digest before sending the manifest's feedback prompt; feedback
contents are not duplicated into the conversation.

## Run an execution

Choose a permanent lowercase execution name. In an external terminal, from
this repository, run:

```text
./oc-run ontology-edsl <exec-name>
```

The command prepares or resumes exactly one temporary workspace and empty
opencode session, then enters the TUI. It does not submit the experiment task.
The TUI and its daemon share that external process lifetime.

After the TUI reports ready, Main starts and observes the execution from
another repository terminal:

```text
./oc-ctl start <exec-name>
./oc-ctl status <exec-name>
./oc-ctl snapshot <exec-name>
./oc-ctl recent <exec-name> 3
./oc-ctl timeline <exec-name> 8
./oc-ctl files <exec-name>
./oc-ctl failures <exec-name>
./oc-ctl audit <exec-name>
./oc-ctl events <exec-name>
```

If the newest assistant step ends with `finish=length`, Main may send the
single fixed recovery message with `./oc-ctl continue <exec-name>`. A completed
`finish=stop` ends one round, not the execution. Main can then ask another
question or relay a Host-written feedback file:

```text
./oc-ctl ask <exec-name> "Question text"
./oc-ctl ask <exec-name> --file /path/to/question.md
./oc-ctl feedback <exec-name> --source-exec <downstream-exec> \
  --source-round 1 --source-message <message-id>
./oc-ctl answer <exec-name> --json
```

Main writes the feedback content to the workspace path printed by
`./oc-ctl workspace <exec-name>`, under `a2/feedback.md`, before invoking
`feedback`.

For an iterative-feedback experiment, keep every participating execution and
TUI alive for the whole feedback loop. `query`, `status`, and the observation
commands are read-only and remain available while a role is actively thinking
or using tools. After that role reaches `idle` with `finish=stop`, Main obtains
its completed answer, decides which observations to relay, updates the target
execution's Host-owned feedback file, and invokes `feedback` there. Repeat this
cycle as needed; do not invoke `finish` between feedback rounds.

## Finish and inspect

When the session is idle and its latest assistant message is a completed
`finish=stop`, freeze the execution while the TUI remains open:

```text
./oc-ctl validate <exec-name>
./oc-ctl export <exec-name>
./oc-ctl finish <exec-name>
```

`export` is an optional diagnostic command that writes a complete raw session
export to `target/exp/<exec-name>/session-export.json`; callers never need to
handle a session ID or invoke `opencode export` directly.

`finish` collects raw messages, normalized query data, declared workspace
paths, all four validation records, a run log, and a summary skeleton under
`target/exp/<exec-name>/result/`. It does not stop the TUI. Exit the TUI after
`finish`; `oc-run` then removes only the verified temporary workspace. Main may
instead run `./oc-ctl retire <exec-name>` after verifying the archive. Exiting
before `finish` preserves the workspace and lets the same `oc-run` command
resume it.

The same query works live and after shutdown:

```text
./oc-ctl query <exec-name> '.summary'
./oc-ctl query <exec-name> --file /path/to/query.jq --raw-output
```

The query engine is selected from `jaq`, `jq`, `mise x -- jaq`, and
`mise x -- jq`.
Set `OC_QUERY_ENGINE=jaq|jq|mise-jaq|mise-jq` for a strict override. Use
`./oc-ctl doctor` to inspect local capabilities. Every external CLI uses the
same `<cli> --version`, then `mise x -- <cli> --version` probing and caches the
successful command prefix. The plan declares its commands without encoding
tool installation details.

Historical `target/opencode-test-*` directories remain immutable evidence of
their original protocol. New executions are never moved into or compared as
verbatim continuations of those archives.
