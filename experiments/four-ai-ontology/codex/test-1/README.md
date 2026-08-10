# Codex test-1 Four-Agent Ontology Experiment

This directory is the repository snapshot of formal run `20260810-161853`. It preserves the
staged inputs, accepted role outputs, bounded feedback, Host validation layouts, handoffs, and
final records that originally lived under ignored `target/` and temporary `/tmp` paths.

The experiment used the `soft-reproducible-v1` profile and an instruction-isolated Main-star
topology. It completed the functional chain with disclosed protocol deviations; it does not claim
adversarial filesystem isolation or strict identity-registry conformance.

## Layout

```text
test-1/
  a1/                  fixed tutorials, role contracts, corpus, and public API
  a2/
    round-0/           initial eDSL delivery
    round-1/           accepted eDSL after bounded verification correction
    FEEDBACK-round-1.md
  a3/                  accepted logistics enterprise model
  a4/                  accepted output for each of the six fixed intent trials
  host-validation/
    stage3/            self-contained Stage 3 validation layout
    direct/            self-contained Stage 4 Host layout
    novel/
    unapproved/
    mixed/
    fanout/
    STAGE4-RESULTS.md
  records/             original manifest, handoffs, requests, feedback, and reports
  RUNLOG.md             chronological experiment narrative
  FINAL-SUMMARY.md      final functional and protocol assessment
  MANIFEST.md           accepted paths, baselines, hashes, and deviations
```

The `impossible` trial has no executable Host directory by design. Its accepted artifact is an
explicit refusal in `a4/impossible/`, and Host classification depends on the absence of an invented
identifier, executable intent, SQL, physical mapping, or hand-built plan.

## Results

| Stage | Result |
|---|---|
| Stage 2 | accepted after bounded correction round 1 |
| Stage 3 | accepted at round 0 |
| Stage 4 direct | `lowered` |
| Stage 4 novel | `lowered` |
| Stage 4 unapproved | `model-rejected` |
| Stage 4 mixed | `model-rejected` |
| Stage 4 fanout | `model-rejected` after one bounded semantic correction |
| Stage 4 impossible | `agent-refused` |

There were no false acceptances or false rejections in the fixed six-trial corpus.

## Rechecking

Use the repository Telora binary from the repository root. Stage 2 probes are self-contained:

```text
target/debug/telora check experiments/four-ai-ontology/codex/test-1/a2/round-1/probe.telora
target/debug/telora run experiments/four-ai-ontology/codex/test-1/a2/round-1/probe.telora
target/debug/telora check experiments/four-ai-ontology/codex/test-1/a2/round-1/analytics_probe.telora
target/debug/telora run experiments/four-ai-ontology/codex/test-1/a2/round-1/analytics_probe.telora
```

Stage 3 and expressible Stage 4 trials use their self-contained Host layouts:

```text
target/debug/telora check experiments/four-ai-ontology/codex/test-1/host-validation/stage3/crates/enterprise-model/valid.telora
target/debug/telora run experiments/four-ai-ontology/codex/test-1/host-validation/stage3/crates/enterprise-model/valid.telora
target/debug/telora check experiments/four-ai-ontology/codex/test-1/host-validation/direct/crates/intent/intent.telora
target/debug/telora run experiments/four-ai-ontology/codex/test-1/host-validation/direct/crates/intent/intent.telora
```

Invalid trials intentionally return fatal diagnostics and nonzero run status. Do not collapse those
expected rejections into infrastructure failures.

## Provenance

- Repository baseline: `3c27d373d6319c09d1a0432a96694b89f333f5ff`
- Original runtime root: `/tmp/telora-builtin-star-20260810`
- Original stable records: `target/exp-recs/20260810-161853`
- Progress log: GitHub Issue #8
- Final status: `completed-with-protocol-deviations`
- Isolation grade: `instruction-isolated`

The accepted source files are copied verbatim. `README.md` and `RUNLOG.md` are repository indexes
written during archival; they are not role outputs.
