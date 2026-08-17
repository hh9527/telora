# RFC 0241: Recursive Type and Value Performance

- Status: Proposed
- Tracking issue: #87
- Depends on: RFC 0034, RFC 0035, RFC 0235, RFC 0237, RFC 0238

## Summary

Telora will make recursive declared types and shared recursive values cost
proportional to the distinct graph actually inspected. A declared identity is
the primary comparison and validation key; structural bodies are traversed only
at boundaries that establish identity or when anonymous structural types are
compared. Graph algorithms retain active-cycle detection and add completed
memoization where a shared DAG is currently expanded as a tree.

This is an umbrella RFC. RFC 0242 establishes reproducible evidence and
regression coverage, RFC 0243 defines declared-identity fast paths, and RFC 0244
defines completed graph memoization. The phase does not change assignability,
Fail propagation, declaration identity, or the authoritative TypeMetadata
model.

## Problem

Issue #83 contains two independent reproductions tracked to completion by #87:

- repeated checks involving nested or recursive descriptors make a realistic
  QueryBuilder take several seconds; and
- a twenty-level value DAG built as `[previous, previous]` takes tens of
  seconds even though it contains only linearly many distinct heap objects.

RFCs 0235 through 0240 made declared Struct and Enum roots nominal, but some
consumers still behave structurally after identity is known. In particular,
declared-value validation revalidates the complete payload, descriptor
normalization can clone a declared body repeatedly, and legacy `Value`
projection detects only active cycles rather than reusing completed nodes.

The Work/Main heap copier is not part of this defect: it installs a forwarding
handle before visiting children and already preserves graph sharing.

## Invariants

The phase preserves these rules:

1. equal `DeclaredTypeId` values denote the same declared type;
2. equal structure never manufactures declared identity;
3. raw, decoded, Host-provided, or otherwise unbranded values are structurally
   validated before a declared wrapper is minted;
4. a value already carrying an unforgeable declared owner may use that owner as
   proof of its payload contract;
5. anonymous Struct, Enum, Tuple, Array, Dict, Union, and Function types remain
   structurally checked;
6. active cycles at the legacy acyclic `Value` boundary remain errors; and
7. memoization may preserve DAG sharing but may not convert an unsupported
   cycle into a successful export.

## Child RFCs

### RFC 0242: evidence and regressions

The existing issue fixtures become a stable, repeatable performance suite. The
suite records medians rather than a single wall-clock sample, separates the
six known shapes, and adds correctness assertions that prevent an optimization
from skipping required raw-value validation.

### RFC 0243: declared-identity fast paths

Runtime validation accepts a matching trusted declared wrapper without
descending into its payload. Type inference and comparison avoid resolving or
formatting a declared body when owner identity or family arguments are
sufficient.

### RFC 0244: completed graph memoization

Structural graph consumers distinguish `visiting` from `completed`. A repeated
completed handle or stable type pair reuses its prior result. The initial scope
is legacy `Value` projection and the remaining structural type-pair paths shown
by RFC 0242 evidence; already-linear heap copying is audited but unchanged.

## Stopping rules

Each child RFC must land with correctness tests and before/after measurements.
An optimization is rejected if it:

- trusts a raw value merely because an expected declared type is available;
- keys correctness on display text, `Debug` output, or heap addresses that do
  not have stable identity semantics;
- suppresses a mismatch that strict execution currently reports;
- changes the successful exported value; or
- hides exponential work by lowering a fixture depth.

## Acceptance criteria

This phase is complete when:

1. all issue #83 fixtures succeed under their existing semantics;
2. the growing recursive-value fixture scales with distinct graph nodes rather
   than its unfolded tree;
3. realistic QueryBuilder `check` and no-match `show` no longer spend seconds
   repeatedly expanding declared descriptors;
4. raw values still require full structural validation before branding;
5. mismatched declared identities remain deterministic errors;
6. cyclic legacy export remains rejected while repeated DAG nodes are reused;
7. tests cover type comparison, validation, copy, publish, and projection
   boundaries; and
8. workspace formatting, linting, and tests pass.
