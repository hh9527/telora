# Telora design discussions

This directory holds design explorations that are not yet RFCs.

A discussion document may establish motivation, compare semantic models, and
record provisional syntax, but it does not commit Telora to an interface or an
implementation. Once the important alternatives and consequences are
understood, the accepted design can move into a numbered RFC with explicit
goals, non-goals, acceptance criteria, and an implementation plan.

Current discussions:

- `intent-compiler-libraries.md`: Telora as a high-level intent carrier and a
  language for versioned domain compiler libraries;
- `intelligent-reporting-intent-compiler.md`: an executable analytics ontology
  lowering report intent into SQL, result schema, and render plans;
- `typed-accumulation-channels.md`: caller-selected typed accumulation;
- `type-directed-capability-factories.md`: deriving typed `Eq`/`Hash`-like
  functions from `TypeOf(A)` without trait resolution; and
- `user-space-type-metadata-interpreters.md`: open-recursion interpreter ABI,
  native/Telora parity, fallback, and reflection gaps; and
- `adversarial-validation-gaps-rank1-inference.md`: completed review inventory
  retained as validation history; and
- `silent-any-generic-inference-audit.md`: completed classification of strict,
  recovery, and Host-boundary `Any` paths after the imported-scheme fix.
