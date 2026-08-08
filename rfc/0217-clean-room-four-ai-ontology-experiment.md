# RFC 0217: Clean-room four-AI ontology experiment

- Status: Accepted
- Depends on: RFC 0216

## Summary

Define a repeatable clean-room experiment that tests whether executable
knowledge can pass through four independently isolated AI roles:

```text
human language design
-> AI-1 implements Telora and writes its tutorial
-> AI-2 learns Telora and implements a supplied ontology eDSL design
-> AI-3 learns both and models a new private enterprise
-> AI-4 expresses queries inside that enterprise's published boundary
-> Telora rejects invalid intent or lowers valid intent to an execution plan
```

The repository contains only stable experiment inputs and this protocol:

- `/tutorial.md`, representing AI-1's public language tutorial;
- `/experiments/four-ai-ontology/edsl-design.md`, the eDSL surface design
  implemented by AI-2;
- `/experiments/four-ai-ontology/domain.md`, the private enterprise brief
  revealed only at the AI-3 stage; and
- RFC 0217, defining roles, isolation, observations, and acceptance criteria.

AI-2, AI-3, and AI-4 outputs are per-run temporary artifacts. They must not
silently become inputs to later repetitions through the repository.

## Research questions

1. Can AI-2 learn enough Telora from its tutorial to implement the supplied
   ontology eDSL design without seeing an enterprise model or implementation?
2. Can AI-2 teach that eDSL to AI-3 through a bounded public tutorial?
3. Can AI-3 model a previously unseen private domain without reading another
   enterprise implementation?
4. Can AI-4 use only AI-3's public intent vocabulary, while remaining unable
   to bypass private physical mappings and policies?
5. Do Telora diagnostics help each role converge without granting it direct
   execution access?
6. When a request is valid, does successful lowering produce a typed,
   executable plan rather than merely a Boolean acceptance result?

## Stable inputs

### Telora tutorial

`/tutorial.md` is the only language-learning document available to AI-2. It
must be usable without README, RFC history, examples, source code, tests, or
prior conversation. Corrections to the tutorial are versioned repository
changes and recorded in the run manifest.

### Ontology eDSL design

`/experiments/four-ai-ontology/edsl-design.md` specifies the method boundary,
definition roles, capability compilation, path semantics, diagnostic behavior,
analytics pipeline, enterprise extension points, and atomic publication
contract. It deliberately omits Telora source, concrete function signatures,
module layout, path algorithm, and teaching structure.

AI-2 must implement this design and choose its surface API. The first run tests
design transmission and implementation quality, not independent invention of
the ontology methodology. Later experiments may progressively remove parts of
the design document.

### Enterprise domain

`/experiments/four-ai-ontology/domain.md` describes a logistics fulfillment
enterprise. AI-2 must not see it. AI-3 receives it together with AI-2's public
eDSL package and tutorial. AI-4 does not receive it directly; it sees only the
intent-facing reference AI-3 deliberately publishes.

## Per-run filesystem

The experiment controller creates one explicit run root:

```text
RUN_ROOT=$(mktemp -d)

$RUN_ROOT/
  manifest.json
  stage-2/
    input/
    output/
    feedback/
  stage-3/
    input/
    output/
    feedback/
  stage-4/
    input/
    output/
    feedback/
  host/
    validation/
    logs/
```

The resolved absolute path is recorded in `manifest.json`; agents are never
given `$RUN_ROOT` as an unresolved environment variable. The controller does
not create the run inside the repository or reuse a previous run directory.

The run manifest records:

- Git commit and dirty-worktree status;
- hashes of `tutorial.md`, `edsl-design.md`, `domain.md`, prompts, and every
  staged input;
- model identity and role prompt for each AI;
- wall-clock start/end for every delivery and feedback round;
- commands executed only by the Host;
- raw Host diagnostics before filtering;
- exact diagnostics returned to the agent; and
- hashes of final temporary outputs.

## Isolation mechanism

Prompt instructions alone are insufficient. Each role receives a physical
staging directory containing only its allowlisted inputs. The role's working
directory is that stage, not the repository.

An agent must not receive:

- the repository path;
- Git metadata or history;
- RFCs, README files, examples, compiler source, or tests;
- another stage's private prompt, logs, or unreviewed output;
- shell, Telora, Cargo, network, or arbitrary filesystem discovery; or
- Host validation code and hidden acceptance queries.

The preferred runner enforces a filesystem allowlist. If the available agent
runner cannot prevent reads outside the stage, the run is marked
`instruction-isolated`, not `filesystem-isolated`, and cannot be used for the
strong clean-room claim.

Agents may create and edit files only in their own `output/`. They cannot
execute or inspect results. The Host copies a delivery into `host/validation`,
resolves temporary dependency paths there, and executes it using the pinned
repository revision.

## Diagnostic relay

The Host never gives an agent shell access as a convenience after failure.
Instead it returns a bounded diagnostic packet:

```text
round
host command class: parse | check | run | recover | hidden acceptance
exit class
diagnostics in stable source order
relevant generated-source excerpts
```

Absolute Host paths, hidden test source, unrelated compiler output, and other
agents' files are removed. Message text, severity, source-relative location,
primary/secondary labels, and provenance chains are preserved.

The agent may edit its output and resubmit. Every round is retained. The Host
must not patch generated code on the agent's behalf.

## Stage 1: AI-1 language implementation

AI-1 is represented by the pinned Telora repository revision and
`tutorial.md`. This experiment does not ask AI-1 to rewrite the compiler on
every run. A run reports the implementation revision and verifies the tutorial
examples before beginning Stage 2.

The claim being tested downstream is limited: AI-1 produced an executable
language and a tutorial sufficient for a new AI to use its documented public
surface. RFC history remains evidence of implementation provenance, not an
input to later roles.

## Stage 2: AI-2 implements the ontology eDSL

### Inputs

AI-2 receives only:

- `tutorial.md`;
- `edsl-design.md`;
- a role brief requiring a faithful, reusable implementation of that design;
- required package/file naming conventions; and
- output and documentation acceptance shapes.

It does not receive the enterprise domain, existing ontology libraries, prior
eDSL source, or another enterprise model.

### Required outputs

```text
ontology-edsl/
  telora-deps.json
  src/...
EDSL_TUTORIAL.md
AI3_CONTRACT.md
STAGE2_DESIGN.md
STAGE2_NOTES.md
```

The eDSL tutorial must explain extension points without using facts from the
hidden enterprise. `AI3_CONTRACT.md` lists exactly what an enterprise model
must define and what the shared eDSL guarantees. `STAGE2_DESIGN.md` records the
surface API and implementation choices AI-2 made where the supplied design
intentionally left freedom.

### Host validation

The Host supplies a temporary neutral micro-model unknown to AI-2. It checks:

- closed model-specific identity types remain precise;
- missing capability and unsafe relationship failures preserve authored
  subjects;
- independent errors survive recovery;
- incomplete evidence cannot publish a plan; and
- valid evidence invokes a typed final builder.

AI-2 may receive diagnostics from this fixture, but never its hidden source.

## Stage 3: AI-3 models a private enterprise

### Inputs

AI-3 receives only:

- `tutorial.md`;
- AI-2's eDSL package;
- `EDSL_TUTORIAL.md` and `AI3_CONTRACT.md`; and
- the staged copy of `domain.md`.

It cannot read Stage 2 prompts/notes, neutral fixtures, prior enterprise
models, or repository examples.

### Required outputs

```text
enterprise-model/
  telora-deps.json
  src/...
PUBLIC_INTENT.md
valid.telora
invalid.telora
STAGE3_NOTES.md
```

`PUBLIC_INTENT.md` is the only enterprise-specific document later shown to
AI-4. It may expose business concepts, legal combinations, and the public
compile entry shape. It must not expose tables, columns, join predicates, or
private physical-plan construction.

### Host validation

Visible validation checks the examples requested by the domain brief. Hidden
validation additionally checks novel combinations, different-grain measures,
fan-out-only dimensions, missing capabilities, and publication atomicity.

The Host separately reviews ontology classification quality. A model does not
pass merely because outputs match if it duplicates or misclassifies facts in a
way hidden by the current algorithm.

## Stage 4: AI-4 expresses query intent

### Inputs

AI-4 receives only:

- the intent-authoring subset of `tutorial.md` selected before the run;
- `PUBLIC_INTENT.md`;
- the enterprise model's public type/interface stubs, excluding bodies; and
- one natural-language business request per trial.

It cannot read `domain.md`, enterprise implementation, eDSL implementation,
physical mappings, SQL, Host plan builder, or acceptance classification.

### Trial classes

The hidden corpus includes:

- legal requests directly suggested by the public vocabulary;
- legal but novel combinations absent from all tutorials;
- an unknown or unapproved capability;
- a grain mismatch;
- a dimension reachable only through fan-out;
- individually legal concepts whose combination is illegal; and
- an impossible request for which refusal is the correct outcome.

AI-4 writes intent Telora. The Host checks and evaluates it through AI-3's
public compile entry. On failure, AI-4 receives normal diagnostic packets and
may retry. It is forbidden to emit SQL or a Host execution plan directly.

### Success classes

Each trial ends as one of:

```text
lowered
    a valid typed execution plan satisfying hidden invariants

correctly refused
    the model rejects an impossible request and AI-4 does not bypass it

incorrectly accepted
    an invalid request publishes a plan

false rejection
    a valid request cannot be expressed or lowered

non-convergent
    the feedback budget expires
```

## Budgets and stopping rules

Default limits per role:

- 30 minutes to first delivery;
- at most 6 diagnostic feedback rounds;
- no more than 2 consecutive rounds with the identical root diagnostic;
- fixed token/model budget recorded in the manifest; and
- no human code edits before classification.

A run stops and is marked contaminated when an agent reads a forbidden input,
executes code, receives hidden acceptance source, or inherits another role's
conversation context.

Human semantic review may report a model-quality defect after executable
validation. The agent may repair it in a separately counted review round, as
in the earlier third-enterprise pilot.

## Metrics

Report at least:

- time to first delivery and accepted delivery for each role;
- parse/type/runtime/semantic feedback rounds;
- visible and hidden acceptance pass rates;
- independent diagnostic recall;
- AI-4 convergence rounds by trial class;
- false acceptance and false rejection counts;
- number of enterprise callbacks that contain knowledge versus mechanical
  forwarding;
- use of erased types or duplicated shared orchestration; and
- contamination/isolation grade.

Do not collapse the result into one success percentage. Language learnability,
eDSL reuse, enterprise modeling, and intent convergence are different claims.

## Acceptance criteria

One run supports the complete story only when:

1. Stage 2 is filesystem-isolated and AI-2 faithfully implements the supplied
   reusable eDSL design without the enterprise domain or an existing eDSL;
2. the neutral model passes both valid lowering and independent-error recovery;
3. Stage 3 is filesystem-isolated and AI-3 passes visible plus novel hidden
   enterprise tests without reading another model;
4. enterprise types remain closed and no interface uses `Any`, `Dyn`, or
   String identity to manufacture reuse;
5. AI-3 publishes an intent surface that hides physical mappings;
6. AI-4 lowers legal direct and novel requests;
7. AI-4 cannot bypass rejection by emitting physical plans or SQL;
8. invalid requests are rejected with provenance-bearing diagnostics;
9. at least one AI-4 invalid trial converges after diagnostic feedback;
10. impossible requests are correctly refused rather than forced through a
    fallback; and
11. the complete manifest permits another controller to reproduce the staged
    inputs, prompts, validation classes, and timing calculation.

## Honest claims

A successful first run demonstrates that this particular language tutorial
and eDSL design can be transmitted to an isolated implementer, then used with
the enterprise domain and query corpus through the remaining stages. It does
not demonstrate that AI-2 independently invented the eDSL methodology, nor
does it prove arbitrary AI models, arbitrary ontologies, production database
safety, semantic correctness of all private facts, or universal convergence.

AI-4's output is an execution plan, not an authorization to execute it. A Host
must still validate the plan shape, enforce permissions, and decide whether it
may affect enterprise data.

## Non-goals

- committing generated eDSLs or enterprise implementations as canonical
  source;
- requiring AI-2 to independently invent the ontology method in the first
  clean-room run;
- letting AI-2 tailor the eDSL to the hidden enterprise;
- comparing model vendors in the first reproducibility pass;
- giving agents shell access to improve benchmark scores;
- treating prompt obedience as filesystem isolation;
- executing real enterprise queries; or
- claiming that refusal is a failure when the requested capability is outside
  the published model.
