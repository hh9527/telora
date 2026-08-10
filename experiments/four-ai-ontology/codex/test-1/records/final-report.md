# Four-AI Ontology Transfer Experiment Report

## Outcome

Run `20260810-161853` completed the functional staged chain:

```text
Telora tutorial
-> reusable ontology eDSL
-> private logistics enterprise model
-> public intent authoring
-> typed plans or policy-preserving rejection
```

Stage 2 was accepted after one bounded verification correction, Stage 3 at round 0, and Stage 4
produced two lowered requests, three model rejections, and one correct agent refusal. There were no
false acceptances or false rejections in the fixed six-trial corpus. The functional result is
accepted; strict protocol conformance is not claimed because A4 had to be rebuilt across Main
sessions under explicit human authorization and method text changed after the run started.

## Stage Results

### Stage 2

Five source modules and two typed probes pass `check`; both probes run. The analytics probe returns
`Some({dimensions: 1, relationships: 1, total: 101})`. Shared orchestration owns capability
lowering, combination, relationship collection/classification/reporting, candidate construction,
and atomic publication. Closed caller types remain precise; no `Any`, `Dyn`, or String identifiers
erase the reusable interface.

Round 0 source behavior was retained. Round 1 added the missing concrete `compile_analytics` probe
and corrected stale verification notes. It did not redesign the API or source implementation.

### Stage 3

All three Telora files pass `check`. The legal fixture runs successfully and produces an Order-grain,
read-only plan with complete safe relationship mappings. The invalid fixture rejects an unavailable
capability. The runner stops at the first fatal diagnostic, so this run does not claim that the same
fixture execution displayed a later fan-out diagnostic. Enterprise callbacks retain grain,
relationship, mapping, and final plan knowledge behind a declaration-only public API.

### Stage 4

The six fixed trials match their expected Host classifications: two `lowered`, three
`model-rejected`, and one `agent-refused`. `fanout` needed one bounded semantic correction after A4
initially refused a request whose identifiers were both public. The correction disclosed only the
public contract and correctly moved policy judgment back to enterprise `compile`. Invalid and
impossible trials emitted no SQL, physical mappings, manually constructed plans, or invented ids.

Detailed commands, diagnostics, and trial results are in `stage-4.md`.

## Runner And Protocol Findings

- Built-in identities belong to a Main collaboration tree and were not addressable after a new Main
  session. The preflight A4 mapping therefore could not survive recovery. Human authorization
  explicitly permitted rebuilding A4 for remaining Stage 4 work. The rebuilt identity was again
  required after later Main-session transitions. This is a disclosed exception to the fixed
  registry and no-replacement rules.
- Main repeatedly intended to call `followup_task` but instead issued batches of no-effect
  `exec true`. Checks showed the relevant trial workspaces still at their recorded baselines or
  unchanged candidate states after every event. The calls did not reach A4, alter artifacts, or
  consume model rounds. They are Main controller orchestration mistakes, not sub-agent, provider,
  authentication, Telora, or workspace failures.
- No external, CLI, service-based, or substitute role runner completed any formal-stage artifact.
  Stage 2 and Stage 3 were not rerun during Stage 4 recovery.
- The RFC runner wording was tightened during a human-requested Stage 4 pause, and final consistency
  review corrected a stale Runbook sentence about per-trial A4 identities. Because the method bundle
  changed after preflight, the run is marked `completed-with-protocol-deviations`.

## Reproducibility And Isolation

- Repository baseline: `3c27d373d6319c09d1a0432a96694b89f333f5ff`
- Runtime root: `/tmp/telora-builtin-star-20260810`
- Stable records: `target/exp-recs/20260810-161853`
- Progress log: GitHub Issue #8
- Execution profile: `soft-reproducible-v1`
- Isolation grade: `instruction-isolated`

Every role/trial used a separate Git workspace with minimal staged inputs. Host scope checks and
input hashes found only role-owned output changes. This is a reproducible staged knowledge-transfer
result under instruction isolation, not adversarial filesystem isolation. The uncommitted protocol
worktree and authorized identity recovery are recorded limitations.

## Remaining Risks

1. Relationship reachability is bounded to eight edges.
2. Safe route selection may retain multiple alternatives rather than a minimal tree.
3. Combined or extra-node diagnostics need enterprise-supplied subjects for equal provenance
   precision, and combination failure prevents later relationship/final-builder diagnostics.
4. Telora normal execution stops at the first fatal diagnostic, limiting same-run diagnostic recall.
5. Built-in identity durability and collaboration tool routing are not deterministic across Main
   session recovery, so strict registry conformance needs a controller mechanism stronger than
   conversational handoff.
