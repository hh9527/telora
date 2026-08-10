# RUNLOG - Codex test-1

- Formal run: `20260810-161853`
- Date: 2026-08-10 (CST)
- Runner: built-in persistent sub-agents with inherited Codex configuration
- Profile: `soft-reproducible-v1`
- Isolation: `instruction-isolated`
- Topology: Main star; Main is the only feedback relay
- Progress channel: GitHub Issue #8

This log is a stable chronological digest of the original Issue events and handoff records. Exact
recovery prompts, hashes, baselines, and final classifications remain under `records/`.

## Preflight

- Main registered fresh, no-history A2, A3, and A4 identities before the formal run.
- All three returned `READY`; the registry and independent workspace roots were recorded.
- A2 workspace baseline: `506e515706030afb1865c7fde52aac46343e74b2`.
- A3 and A4 remained idle while Stage 2 began.

## Stage 2 - Shared Ontology eDSL

### Round 0

- A2 received only the Telora tutorial, eDSL design, fixed role contract, staged binary, and owned
  crate.
- The delivery contained five source modules, a neutral probe, public tutorial/contract, design
  notes, and verification notes.
- Host scope and type-erasure audits passed. Source and the existing probe checked successfully;
  the probe ran and printed complete capability evidence.
- Host did not accept round 0 because there was no concrete typed probe exercising the shared
  `compile_analytics` orchestration entry, and the notes still claimed probes could not run.

### Bounded correction round 1

- Main wrote one bounded feedback packet, preserved the accepted API/source behavior, and created a
  separate correction workspace from the frozen round-0 candidate.
- The same A2 role history added `analytics_probe.telora` and corrected `STAGE2_NOTES.md`; shared API
  and source modules were not redesigned.
- Five source modules and both probes passed `check`.
- `probe.telora` ran successfully with complete capability evidence.
- `analytics_probe.telora` ran successfully and returned
  `Some({dimensions: 1, relationships: 1, total: 101})`.
- Host accepted round 1 and froze it under `a2/round-1/`.

## Stage 3 - Logistics Enterprise Model

- A3 received the accepted eDSL, its public tutorial/contract, the private logistics domain, the
  Telora tutorial, and its fixed role contract.
- A3 delivered a typed enterprise model, legal and invalid fixtures, notes, and `PUBLIC_INTENT.md`.
- All three Telora source/fixture files passed `check`.
- The legal fixture ran successfully and produced an Order-grain, read-only plan retaining complete
  safe relationship mappings.
- The invalid fixture rejected an unavailable capability. Normal execution stopped at the first
  fatal diagnostic, so the run did not claim that this same invocation also displayed the later
  fan-out diagnostic.
- Host accepted Stage 3 at round 0, froze it under `a3/`, and derived declaration-only
  `a1/PUBLIC_API.md` for A4.

## Stage 4 - Fixed Intent Corpus

Each trial used a new Git workspace, one request, the fixed AI-4 role, the public intent tutorial,
`PUBLIC_INTENT.md`, and declaration-only `PUBLIC_API.md`. A4 did not receive the enterprise or eDSL
implementation and did not run Telora. Host validated expressible intents in separate directories.

### direct

- The first attempts to assign A4 were mistakenly routed by Main as no-effect `exec true` calls.
  The workspace remained at baseline and A4 received no task.
- A Main-session transition made the preflight identity `/root/stable_a4` unreachable. Human
  authorization allowed a rebuilt built-in A4 identity for the remaining Stage 4 work.
- Host check succeeded with 7 dependencies. Run returned a read-only Order plan for
  `OrdersCreated` grouped by `OriginRegion` and `CarrierName`, retaining the required safe mappings.
- Classification: `lowered`.

### novel

- The same rebuilt A4 history received a new independent workspace.
- Host check succeeded with 7 dependencies. Run returned a read-only PackageItem plan for
  `UnitsShipped` grouped by `ProductCategory` and `OriginRegion` with complete safe mappings.
- Classification: `lowered`.

### unapproved

- Main again produced no-effect `exec true` calls before the real collaboration assignment. The
  workspace was unchanged and the event consumed no model round.
- Host check succeeded. Run exited 1 at `DeliveryException` with
  `no capability is defined for the requested id`; no plan was published.
- Classification: `model-rejected`.

### mixed

- Another batch of Main no-op orchestration calls preceded the real assignment without changing the
  workspace.
- Host check succeeded. Run exited 1 with
  `measures at different natural grains require an explicit pre-aggregation policy`; no plan was
  published.
- Classification: `model-rejected`.

### fanout

- Stage 4 paused on human instruction while RFC 0217 was tightened to forbid external, CLI,
  service-based, and substitute runners.
- After a Main-session transition, human authorization allowed rebuilding the built-in A4 identity
  and resuming from the prepared fanout workspace. Earlier stages and trials were not rerun.
- A4 initially refused the request because `ProductCategory` required unavailable expansion policy.
- Host rejected that authoring decision: both `OrdersCreated` and `ProductCategory` are members of
  the public closed vocabulary, so A4 must express the intent and leave policy judgment to
  enterprise `compile`.
- Main supplied one bounded semantic correction containing only public-contract facts. Several
  intended collaboration calls were first misrouted as no-effect `exec true`; the candidate and
  inputs remained unchanged until the real correction reached A4.
- Corrected Host check succeeded. Run exited 1 with
  `relationship expands the measure grain; define explicit pre-aggregation or allocation policy`;
  no plan was published.
- Classification: `model-rejected`, after one bounded semantic correction.

### impossible

- Main repeatedly misrouted the intended assignment as no-effect `exec true` calls. The prepared
  workspace remained clean at baseline `a72dc2ed6a0e1e1460422fca05faa2513ce5193c`.
- A later Main session rebuilt A4 under the recorded human authorization and delivered only the
  fixed impossible assignment.
- A4 explicitly refused because the closed public `MeasureId` lacks average delivery duration and
  `DimensionId` lacks weather condition.
- Host verified that the artifact contained no executable intent, invented identifier, SQL,
  physical mapping, or hand-built plan.
- Classification: `agent-refused`.

## Finalization

- Functional result: 2 `lowered`, 3 `model-rejected`, 1 `agent-refused`.
- False acceptances: 0. False rejections: 0.
- Stage 2 bounded correction rounds: 1.
- Stage 4 bounded semantic correction rounds: 1 (`fanout`).
- Accepted artifacts matched their frozen workspaces, role-owned scope, and staged input hashes.
- YAML corpus classifications, execution-profile isolation labels, text records, and whitespace
  checks passed.
- Issue #8 task list and final comments were updated.
- Final status: `completed-with-protocol-deviations`.

## Disclosed Deviations

1. Built-in identities did not survive Main-session transitions as addressable collaboration-tree
   nodes. A4 was rebuilt under explicit human authorization, violating strict no-replacement
   registry conformance.
2. Main repeatedly selected `exec true` instead of the intended collaboration operation. These
   calls had no effect, left trial workspaces unchanged, and are controller orchestration mistakes,
   not model, runner, API, authentication, Telora, or workspace failures.
3. Runner wording in RFC 0217 changed during a human-requested Stage 4 pause, and a stale Runbook
   sentence was corrected during final consistency review. The method bundle therefore changed
   after preflight.
4. The workspaces were instruction-isolated, not protected by an adversarial operating-system read
   allowlist.
