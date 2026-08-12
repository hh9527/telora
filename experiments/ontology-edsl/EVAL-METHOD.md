# Ontology eDSL evaluation method

This is a Host-only evaluation protocol. It must not be injected into A2's
`a1/` input or quoted in a way that reveals hidden fixtures or implementation
strategies.

## Evaluation question

Can an isolated model create a reusable, precisely typed ontology eDSL from the
stable Telora tutorial and observable design contract, and can it converge from
ordinary compiler/runtime feedback without Host-authored code?

The evaluation distinguishes specification comprehension, eDSL design,
language usability, standard-library adequacy, diagnostics, and runner
reliability. A passing final program does not erase process friction.

## Controlled inputs

Before a run, record:

- hash of `experiment.json` and confirmation that its exact `prompts.start`
  UTF-8 value was used as the initial user prompt without a wrapper;
- hashes of injected `TELORA-TUTORIAL.md` and `EDSL-DESIGN.md`;
- repository revision and dirty-worktree state;
- Telora binary revision;
- model name and configuration;
- runner configuration and any runner-level system instructions;
- this evaluation-method revision; and
- Host fixture revision.

Runs are directly comparable only when these controlled inputs match. A retry
after infrastructure failure is recorded but does not count as an A2
correction.

## A2 procedure

A2 reads only `a1/` and `a2/`, writes only the paths declared by
`experiment.json`, and executes only its four fixed Telora wrappers. Start one
recoverable model session with the manifest's `prompts.start` value verbatim
and retain that session for the stage. Run-specific task text must not be
appended to the initial prompt.

The Host validates deliverables in this order:

1. semantic-role types and exports;
2. capability and path library;
3. compilation pipeline;
4. enterprise-facing tutorial and contract; and
5. hidden end-to-end fixtures.

For each correction, record the authored revision, commands run, complete
diagnostics, feedback sent, response, files changed, and outcome. The Host
relays observations, not solutions. Do not name an algorithm or suggest code.

Budget: at most six diagnostic correction turns per deliverable and two hidden
acceptance correction turns. Two consecutive turns with the same unresolved
root cause are recorded as stuck rather than silently expanding the budget.

## Static acceptance

Check at least:

- no `Any`, `Dyn`, native declaration, or String identity escape;
- precise generic schemes at public boundaries;
- role families and classification types are exported and consumable;
- all imports resolve from the declared experiment environment;
- direct generic products remain precise;
- expected rejection is represented in the result value;
- selected relations reach the builder with their mapping payload; and
- authored subjects survive into structured diagnostics.

## Hidden behavioral matrix

Use domain-neutral names and mappings unknown to A2. The fixed matrix includes:

| Scenario | Required observation |
|---|---|
| No targets, non-empty safe catalog | selected edges are empty |
| One direct target | exactly its edge and mapping reach the builder |
| Unrelated reachable branch | branch is excluded |
| Multi-hop target | all selected path edges arrive base-to-target |
| Equal shortest paths | catalog-order tie-break is stable |
| Longer alternative | shortest path wins |
| Multiple targets with shared prefix | shared edges are emitted once |
| Cycle | terminates and selects only the required path |
| Exactly eight edges | accepted without truncation |
| Reachable only beyond eight | no publication and truncation is observable |
| Safe/fan-out/safe chain | classified fan-out-only, not missing |
| Missing target | classified missing with authored subject |
| Empty or failed capability | aligned evidence retained; no partial plan |
| Unauthorized capability | value-level rejection, not accidental runtime failure |
| Builder rejection | distinct from capability and path rejection |
| Overlapping catalogs | sourced catalog diagnostic |

Run direct path-classifier checks as well as full-pipeline checks. A compiler
that discards selected edges can otherwise mask an incorrect classifier.

## Process measures

Record without collapsing them into a single score:

- initial writes and correction turns by deliverable;
- wall-clock and model turns, excluding infrastructure retries;
- number and locality of compiler diagnostics;
- repeated syntax facts despite their presence in the tutorial;
- use of aliases, CPS, helper extraction, or type erasure;
- public generic arity and API complexity;
- whether Host feedback identified only symptoms or leaked a solution;
- hidden cases passed before and after feedback; and
- deviations between A2 documentation and executable behavior.

## Failure attribution

Classify each root cause using the narrowest supported category:

- **Input contract**: observable behavior was ambiguous or contradictory.
- **eDSL design**: the behavior was specified and expressible, but the chosen
  algorithm/API did not satisfy it.
- **Tutorial discoverability**: a required language fact was missing or hard to
  locate.
- **Language surface**: syntax or composition imposed repeated accidental
  friction.
- **Type system**: a precise valid relationship could not be inferred or
  checked without an unnatural workaround.
- **Standard library**: the language could express the algorithm, but missing
  reusable collection operations dominated it.
- **Diagnostics**: failure was local but reported misleadingly or too late.
- **Infrastructure**: runner, network, process, or session failure independent
  of authored code.

Do not label an eDSL logic error as a language defect merely because a richer
library could have prevented it. Promote a language issue only when the same
minimal pattern reproduces outside the experiment and materially affects
convergence or forces semantic weakening.

## Reporting

`RUNLOG.md` is chronological evidence. `SUMMARY.md` reports:

- controlled-input identity;
- convergence by deliverable;
- final static and behavioral matrix;
- documentation/implementation mismatches;
- root-cause attribution with minimal reproductions;
- comparison only against compatible prior runs; and
- proposed language or protocol work, with confidence and non-goals.

Historical outputs are not repaired after evaluation. A later input revision or
language change receives a new run directory.
