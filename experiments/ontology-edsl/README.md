# Ontology eDSL experiment

This directory is the stable source anchor for the ontology eDSL creation
experiment. It separates versioned experiment inputs from per-run artifacts.

## Files

- `A2-PROMPT.md` is the exact initial prompt passed to the eDSL implementer.
- `TELORA-TUTORIAL.md` is the bounded language tutorial supplied to the eDSL
  implementer.
- `EDSL-DESIGN.md` is the normative, domain-neutral behavior contract supplied
  to the eDSL implementer.
- `EVAL-METHOD.md` defines the Host-side process and outcome evaluation.
- `README.md` defines how an experiment run is prepared and archived.

Only `TELORA-TUTORIAL.md` and `EDSL-DESIGN.md` are injected into a run's `a1/`
directory. The runner passes the exact UTF-8 contents of `A2-PROMPT.md` as the
initial user prompt, without a prefix, suffix, or run-specific interpolation.
`EVAL-METHOD.md` remains Host-only: exposing its hidden acceptance fixtures or
evaluation guidance to A2 would change the experiment.

## Run layout

Each run uses a fresh directory under `target/`:

```text
target/opencode-test-N/
  a1/
    TELORA-TUTORIAL.md
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

At the start of A2, the Host copies the two injectable documents from this
directory into `a1/` without editing them and starts one recoverable session
with `A2-PROMPT.md` verbatim. The Host records all three content hashes, the
Telora revision, model identity, runner configuration, and evaluation-method
revision in `RUNLOG.md`.

There is no separate A2 protocol or role-brief file. The stable prompt owns the
role, filesystem boundary, and completion instruction; the tutorial owns
language facts; the design document owns observable eDSL behavior. Duplicating
those responsibilities in another A2-visible file would create a second task
definition.

Historical `target/opencode-test-*` inputs and outputs are immutable evidence.
Changes to this anchor apply only to later runs. When the stable input changes,
the next run must identify it as a new input revision rather than claiming
verbatim comparability with an older run.

## Experimental boundary

A2 may read only `a1/` and may write only `a2/`. It must not read repository
ontology examples, earlier experiment outputs, Host fixtures, or this
Host-only evaluation method. A2 authors the eDSL independently and cannot run
Telora or Cargo; the Host executes checks and relays observations.

The Host may report:

- the command category that failed;
- source diagnostics with locations;
- the hidden scenario name or behavior under test;
- expected and actual observable values; and
- whether a failure is static, runtime, diagnostic, or protocol-level.

The Host must not provide a reference implementation, algorithm name,
pseudocode, or a patch. A2 remains responsible for design and correction.

## Interpretation

The experiment evaluates whether an isolated model can create a reusable,
typed ontology eDSL from stable language and behavior specifications. It does
not measure memorization of Telora syntax, and it does not prove that one model
or one successful run generalizes to other domains.

Results are reported separately for language learnability, eDSL contract
compliance, enterprise extensibility, diagnostic quality, convergence, and
boundary preservation. They are never collapsed into a single pass rate.
