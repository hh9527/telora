# RFC 0011: Execution Quotas

- Status: Accepted
- Implementation: Complete

## Summary

This RFC introduces one quota model for every XL execution. A quota bounds
evaluation fuel, peak XL stack slots, and cumulative heap allocation requests.
The VM and native functions see only the quota attached to their
`CallContext`; they neither know nor branch on why an execution was started.

The embedding API temporarily provides two policies:

```rust
pub struct EngineConfig {
    pub module_quota: Quota,
    pub session_quota: Quota,
    pub data_limits: DataLimits,
}

pub struct Quota {
    pub fuel: usize,
    pub stack_slots: usize,
    pub allocation_bytes: u64,
}
```

Each XL module initialization receives a fresh account copied from
`module_quota`. Invoking a loaded module as a runtime entry point receives a
fresh account copied from `session_quota`. Quota configuration is Rust API only
in this RFC; XL syntax, manifests, and command-line flags are deferred.

## Motivation

RFC 0010 bounds dynamic evaluation but deliberately does not bound stack
occupancy or allocation volume as one execution policy. RFC 0009 has global
structural stack constants, but a host cannot give separate workloads smaller
limits. In addition, module loading currently gives each tool expression a new
fuel allowance, so a module's initialization work has no single account.

XL's closed-world module model gives a useful ownership boundary. A module is
conceptually a zero-argument pure function evaluated once. Its result is
published in a module registry and remains rooted in the VM heap. Initialization
tasks may have independent stacks and accounts even though their resulting
values share a heap. A later invocation of an exported entry function is a new
execution task, not a continuation of the module initialization account.

This RFC establishes that boundary before introducing a centralized or moving
heap. The current `Arc` representation remains an implementation detail; all
VM-visible allocations must nevertheless pass through the execution account.

## Execution boundary

`Quota` is an immutable limit definition. Every execution creates a mutable,
private account:

```rust
pub struct QuotaAccount {
    quota: Quota,
    remaining_fuel: usize,
    requested_allocation_bytes: u64,
}
```

Stack occupancy is measured from the execution's XL value stack and therefore
does not need a second mutable counter. The VM checks its current and requested
slot count against `quota.stack_slots` whenever it creates a bytecode frame or
extends a native window.

The conceptual native boundary is:

```rust
pub struct CallContext<'vm, 'stack> {
    vm: &'vm mut Vm,
    stack: &'stack mut ValueStack,
    quota: &'stack mut QuotaAccount,
    // register-window metadata
}
```

Native callbacks continue to access XL values only through registers. Value
construction methods on `CallContext` charge the same account as bytecode
instructions. No module/session discriminator and no general-purpose fuel
charging method are exposed to native code.

## Account lifetime

Every module has an independent initialization account:

```text
module_quota
-> fresh account and stack
-> tool-stage and module-value evaluation
-> publish module result
-> discard account and stack
```

The allowance is shared by all tool expressions and runtime evaluation needed
to initialize that one module. It is not reset for each type declaration or
binding. Imported modules pay for their own initialization and do not consume
the importing module's quota.

Every entry invocation has an independent session account:

```text
session_quota
-> fresh account and stack
-> invoke loaded entry
-> return or fail
-> discard account and stack
```

The root module is ordinary during initialization. Its initialization uses
`module_quota`; executing its result uses `session_quota`. Multiple sessions
receive independent accounts.

The MVP loader remains sequential. The account model must not depend on this:
independent branches of a future static module DAG may initialize concurrently,
because they share neither stack nor mutable quota state. Module results may
share the VM heap and intern tables.

## Fuel

The `fuel` field follows RFC 0010 exactly. Calls and taken control-flow edges
whose target is less than or equal to the current PC consume one unit. Ordinary
instructions, forward edges, and returns consume none.

All nested bytecode and native calls in one execution consume the same account.
A native callback has no API for charging work proportional to bytes or items;
such work is bounded by admitted data and allocation limits rather than a
virtual instruction price.

## Stack quota

`stack_slots` is a peak concurrent occupancy limit, not a cumulative charge.
Bytecode frame registers, arguments, upvalues, native result slots, and native
scratch registers all occupy the same continuous XL value stack and count
toward the limit. Returning or truncating a native window releases slots.

RFC 0009's engine structural maximum remains an unconditional safety ceiling.
The effective stack limit is:

```text
min(execution quota stack_slots, engine maximum stack slots)
```

Call depth remains a separate structural limit because a small-register
recursive function can exhaust frame metadata before stack slots.

## Allocation quota

`allocation_bytes` bounds cumulative allocation requests performed while an
execution is active. It is monotonic: dropping or replacing an XL value does
not refund bytes. The check occurs before allocation; a failed charge performs
no requested allocation and returns `AllocationQuotaExceeded` at the allocating
instruction or native call.

The accounting unit is logical payload size, independent of allocator layout:

```text
String       UTF-8 byte length
Bytes        byte length
Array        item count * size_of::<Value>()
Tuple        item count * size_of::<Value>()
Dict shape   sum of field UTF-8 bytes
Dict values  field count * size_of::<Value>()
Closure      capture count * size_of::<Value>()
```

Container header, `Arc` header, capacity rounding, allocator metadata, and
interner hash-table overhead are not charged. Dict shape bytes are charged for
every dict construction request even when shape interning reuses an existing
shape. This makes accounting deterministic and independent of cache state or
parallel initialization order.

Primitive `Int`, `Float`, and built-in atoms allocate no payload. Loading a
pre-existing bytecode constant and cloning an immutable value allocate no new
XL payload and therefore do not charge. Bytecode prototypes, constant pools,
source text, AST/CST nodes, imported JSON parsing, and host-provided input are
admission or compiler resources outside execution allocation quota.
Static JSON/YAML/TOML modules and Entry `data_srcs` instead use independent
`DataLimits` admission bounds for raw file size and validated logical graph
shape. Their validation and Heap materialization do not consume VM fuel, stack,
or allocation quota.

Checked arithmetic is mandatory for every size calculation and account update.
Overflow is treated as quota exhaustion.

## API surface

`EngineConfig` is the only place in this RFC that distinguishes module and
session policy. An `Engine` owns that configuration and applies it at the
corresponding host boundary. The CLI uses explicit built-in defaults.

The low-level VM accepts `Quota`, creates an account, and executes without any
module/session concept. Compatibility helpers that accept only fuel may remain
temporarily, but all production module and CLI paths must use quota-aware APIs.

Quota failure uses distinct runtime kinds:

```text
FuelExhausted
StackLimitExceeded
AllocationQuotaExceeded
```

All retain the existing debug origin and frame trace. A module initialization
failure is fatal to that load operation and names the responsible module. No
allocation-account rollback or failed-world reuse is required.

## Atoms

There is currently no VM-owned atom table and no dynamic `String -> Atom`
operation. Named atoms embedded in bytecode are static compiler artifacts.
This RFC therefore does not expose atom count or atom-byte fields that could
not be enforced honestly.

A future atom interning RFC may extend `Quota` with count and byte limits.
Static atoms should be collected and interned deterministically for the closed
world, while dynamic atom requests must be charged independently of cache-hit
order.

## Hard limits and admission limits

The following remain engine/compiler constants rather than execution quota:

- source and data-file byte limits;
- module count and total module-graph input size;
- parser node count, nesting depth, and collection cardinality;
- instructions and constants per prototype and per compilation;
- maximum register index, frame depth, and engine stack slots.

Execution quota is not permission to exceed a structural hard limit. Input and
compiler admission limits are deferred to a separate RFC.

## Rejected alternatives

### Give `CallContext` module and session modes

This leaks host policy into the execution ABI and makes built-ins behave
differently for identical XL calls. Account construction belongs above the VM.

### Use a shared quota across all modules

The result would depend on module initialization order and would inhibit
deterministic parallel initialization. Per-module accounts also identify the
module responsible for exhaustion directly.

### Refund allocation when values die

The current runtime has no centralized heap ownership or reliable attribution
of shared values. Cumulative requests are simple, deterministic, and limit
allocation churn as well as retained output.

### Charge physical allocator bytes

Allocator layout, `Arc` headers, capacity growth, and platform word size would
make language resource behavior implementation-dependent. Logical payload
accounting is stable enough to validate the model now.

### Count constants and imported data as runtime allocation

Those values exist before the execution that reads them. Charging on access
would double-count shared immutable values and make a cheap constant load look
like allocation. Their sizes belong to compilation and input admission limits.

### Add manifest or XL quota declarations now

The purpose of this RFC is to validate execution isolation and accounting.
Configuration syntax would prematurely commit policy and module metadata
semantics. A future declaration must be statically known before initialization
and approved against host maxima.

## Deferred work

- a shared centralized heap and handles suitable for moving GC;
- a persistent `ModuleRegistry` with explicit initialization states;
- parallel scheduling of the closed-world module DAG;
- atom-table quotas and deterministic static atom pre-interning;
- configurable manifests, module declarations, and CLI flags;
- input, parser, compiler, and total-world admission limits;
- host-wide physical memory ceilings, deadlines, and cancellation;
- multiple named entry points and long-lived runtime sessions.

## Implementation plan

1. Add public `Quota`, `EngineConfig`, and `Engine` API types plus internal
   `QuotaAccount` state.
2. Make low-level VM execution create one account and thread it through every
   bytecode frame and native `CallContext`.
3. Replace fixed stack checks with the effective per-execution stack limit
   while retaining RFC 0009's structural ceiling.
4. Centralize checked allocation charging and route bytecode/native creation of
   strings, arrays, tuples, dicts, and closures through it.
5. Make all tool expressions for one module share its initialization account;
   give each imported XL module a fresh module account.
6. Apply `EngineConfig.module_quota` during loading/initialization and
   `EngineConfig.session_quota` when executing a loaded root module.
7. Add source-aware boundary tests for each quota dimension, account isolation,
   cumulative allocation, shape-cache independence, and module/session policy.

## Acceptance criteria

1. VM and `CallContext` contain no module/session mode or quota-source branch.
2. Calls and back edges consume shared fuel exactly as RFC 0010 specifies.
3. One execution's nested calls share fuel and allocation counters.
4. Stack quota counts bytecode and native slots on their common XL stack and
   releases occupancy on return.
5. Runtime construction of every allocating value kind is charged before the
   allocation using the documented logical size.
6. Allocation charges are cumulative and are not refunded when values become
   unreachable.
7. Dict allocation charges are identical on shape-interner hits and misses.
8. Allocation, stack, and fuel exhaustion have distinct source-aware errors.
9. Every XL module initialization receives a fresh module account; tool-stage
   expressions inside that module do not receive fresh accounts.
10. The root module uses module quota while loading and a fresh session quota
    while executing.
11. Two runtime invocations receive independent session accounts.
12. Existing language results are unchanged when quotas are sufficient.
13. Workspace tests, strict Clippy, formatting, and diff checks pass.

## Implementation result

The public API now provides `Quota`, `EngineConfig`, and `Engine`. The CLI uses
explicit built-in module and session policies, while compatibility entry points
that accept only evaluation fuel translate to an otherwise unrestricted quota.
The VM creates one `QuotaAccount` per execution and threads it through nested
bytecode frames and the native register window without exposing whether the
account came from module initialization or a runtime session.

Fuel remains governed by RFC 0010. Stack frame creation, native windows, and
scratch registers use the quota's peak slot limit capped by the engine hard
maximum. Runtime string interpolation, arrays, tuples, Dict shapes and values,
closures, and corresponding native construction methods charge deterministic
logical payload bytes before constructing the final XL payload. Allocation
exhaustion has its own runtime kind and preserves normal debug origins and
traces. Dict tests demonstrate identical charges for interner hits and misses.

All tool expressions in one XL module now share its account. Each imported XL
module gets a fresh module account, and its tool evaluation and module-value
evaluation share that account. The root module uses a module account while it
is loaded and analyzed; executing its compiled entry receives a fresh session
account on every invocation. Tests independently exhaust module fuel and
session allocation, and prove that repeated sessions do not share counters.

The existing runtime still represents heap values with `Arc` and initializes
the root source as a compiled entry rather than publishing a module value in an
explicit `ModuleRegistry`. The quota ABI does not depend on those transitional
choices. A shared heap/registry and the later `module value -> exported entry`
split remain deferred as stated above.
