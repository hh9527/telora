# RFC 0066: Static data publication pipeline

- Status: Implemented
- Depends on: RFC 0057, RFC 0065

## Summary

Forma publishes JSON, TOML, and future static data formats through one module
pipeline. Each format retains its own lossless frontend and semantic lowerer,
but returns the same boundary result: an optional `SourcedValue`, diagnostics,
and a workspace module kind.

```text
resolved module
  -> registered source
  -> format frontend
  -> SourcedValue + diagnostics
  -> persistent module cache
  -> workspace snapshot
```

This is a publication contract, not a generic parser abstraction. JSON and
TOML do not share tokens, CST rules, recovery policy, or lowering machinery.

## Motivation

The strict loader and recoverable workspace originally repeated a branch for
every static format. That made cache insertion, unavailable-module behavior,
and workspace publication easy to vary accidentally as YAML and later formats
arrived. The important commonality begins after resolution and source
registration, where all static formats produce one immutable value graph with
source provenance.

## Contract

A static frontend receives a registered `SourceId` and returns:

- a value with key, container, element, and scalar provenance when lowering
  succeeds;
- all syntax or semantic diagnostics;
- no guessed or partial runtime value when diagnostics prevent publication;
- the distinct workspace kind for tooling.

The recoverable loader retains source and diagnostics for invalid data. The
strict loader renders those same diagnostics and fails. A successful value is
promoted once and cached by resolved `ModuleId`; path aliases therefore cannot
produce separate runtime values.

Cancellation remains a concern of the asynchronous workspace traversal. A
format parser may add internal checkpoints for expensive work, but the shared
publication boundary does not prescribe spawning or threading.

## Non-goals

- a shared JSON/TOML/YAML grammar or token model;
- a broad parser trait or dynamic format registry;
- partial runtime values from malformed static data;
- decoding imported data directly to a user schema;
- changing module resolution or format selection.

## Acceptance criteria

1. recoverable JSON and TOML loading use one source/publication path;
2. strict JSON and TOML loading use the same frontend dispatch contract;
3. invalid modules retain their format-specific workspace kind and diagnostics;
4. successful values retain provenance and resolved-identity caching;
5. format CSTs and lowerers remain independent;
6. existing JSON and TOML behavior and diagnostics remain unchanged.

## Implementation result

`parse_static_data_registered` is the single format dispatch boundary for the
strict loader and workspace tools. `WorkspaceBuilder::load_static_data` now owns
source acquisition, snapshot availability, diagnostics, and value-cache
publication for every supported static format. The strict loader uses the same
parse result before importing the sourced value into the persistent heap.

JSON and TOML still expose their concrete parse products to syntax tooling and
tests. The shared result deliberately erases only their CST type after lowering
has completed.
