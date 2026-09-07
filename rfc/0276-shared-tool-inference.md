# RFC 0276: Shared Tool Inference and Static Property Evidence

- Status: Proposed; local performance experiment.
- Baseline: 58f0b8c, after RFC 0274.
- Related: RFC 0260, RFC 0274, RFC 0275.

## Motivation

RFC 0274 introduced constructor inference before each tool expression. Each
invocation copies module environments, decodes every tool binding and rebuilds
declaration evidence. A debug check of 400 independent struct declarations
increased from 3.08 s before RFC 0274 to 11.46 s. Instrumentation attributed
7.58 s to tool inference, including 362,085 type-decoding attempts.

The module already has an authoritative inference pass. Ordinary value bodies
should be checked there; only dependencies of metadata computations need early
tool evaluation. Type identities and declaration contracts are shared module
facts, not inputs that must be reconstructed for each expression.

## Design

Maintain declaration contracts and bodies as module-level inference inputs.
Publish newly established bindings incrementally. Tool expressions use only
their referenced value inputs, including lexical generic witnesses, while
sharing module contracts and declaration identity. Failed or incomplete local
inference must not contaminate later expressions or publish unresolved evidence.

Keep the full-module constraint pass authoritative. Retain complete constructor
evidence when compiling an already checked tool expression. Preserve checks
for unknown members, invalid generic arguments and nominal payload contracts.

Property providers cannot modify the type skeleton. Their return contracts
determine the property type. Register type-property evidence and its future
runtime binding from those contracts before module inference. Trait and
Property constraints can consume this static evidence without evaluating the
property value. Lexical evidence for generic parameters follows the same rule.

After static checking succeeds, evaluate property providers and materialize
their records. A failed provider, invalid capability or missing runtime record
prevents module publication; static evidence alone never authorizes publication.
Retain member-before-type evaluation, previous-property values and provenance.
Static type errors may now precede property-provider execution errors.

## Non-Goals

- Changing constructor, trait-selection or property applicability rules.
- Caching results solely by source location across different lexical scopes.
- Removing checks or treating incomplete metadata as successful evidence.
- Implementing RFC 0275 construction checks.
- Publishing, pushing or merging this local experiment.

## Verification

Use language cases for local and imported constructor providers, generic
shadowing, property-constrained traits, provider failures and diagnostics.
Run the workspace suite with a debug build. Compare identical declaration,
function and shared-type workloads against the preserved baseline executable.
Keep benchmark inputs and a reproducible runner with the change. Record actual
results and remaining architectural limitations before marking implemented.
