# RFC 0177: Entry runtime and virtual modules

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Entry run/serve receive an explicit State witness and _rt.state_type is removed.
- Depends on: RFC 0176

## Summary

Add the privileged native module `entry/rt.native.forma`. It exposes direct
Host observations and mutation of the pending module's invocation-local
virtual module registry. Every stateful operation receives `ModuleHandle`;
there is no ambient current entry.

```forma
native type ModuleHandle = @1;
native type InstantiatedModule = @2;

native def args: Fn(ModuleHandle) -> Array(String);
native def cwd: Fn(ModuleHandle) -> String;
native def var: Fn(ModuleHandle, String) -> Option(String);
native def platform: Fn(ModuleHandle) -> Platform;
native def cache_root: Fn(ModuleHandle) -> String;
native def inject_module:
    for(A) Fn(ModuleHandle, String, TypeOf(A), A)
        -> Result(Bool, BlameError);
native def initialize_module:
    Fn(ModuleHandle) -> Result(InstantiatedModule, BlameError);
```

The selected entry may wrap these explicit-handle functions in closures for a
more concise local API or deliberately delegate them to main.

## Host observations

`args`, `cwd`, `var`, platform, and cache paths read Host state when called and
materialize ordinary Forma values. No separate InvocationContext snapshot is
required: entry itself is the open-to-closed boundary, and main observes only
the values or capabilities entry chooses to pass.

Missing environment variables produce `None`; non-UTF-8 names or values return
structured blame rather than lossy text. This RFC adds no environment
enumeration API.

## Typed virtual modules

`inject_module(handle, id, TypeOf(A), value)` installs one complete module
value and its authoritative Forma interface into the pending registry. The
module ID is normalized by the ordinary resolver rules. Injection:

- accepts closed and functional values, not only serializable literals;
- preserves native functions, closures, opaque values, and provenance through
  normal heap publication;
- rejects registered built-ins, `entry/`, `@main`, relative IDs, duplicates,
  and invalid/private suffixes;
- is visible only to the main graph owned by the same handle;
- succeeds only while lifecycle state is `Pending`.

`True` reports successful installation. The explicit type witness is decoded
by Forma's metadata decoder and becomes
the injected module's interface. Runtime validation confirms the supplied
value is assignable to that witness before registry mutation.

## Freeze

`initialize_module` atomically takes the registry and changes lifecycle state
before loading main. Main resolution checks invocation-local modules before
workspace dependencies but after immutable registered core/std modules, so an
entry cannot shadow a built-in. Initialization success or failure permanently
closes injection for the handle.

## Privilege

Ordinary source cannot import `entry/rt.native.forma`. Only the exact selected
entry root resolves it. This import restriction controls initial acquisition;
entry may pass any runtime function into main as an ordinary value.

## Acceptance criteria

1. entry runtime native types have stable reserved IDs;
2. handles are identity-opaque and cannot be forged;
3. Host reads require the owning handle and return typed values;
4. typed injected modules resolve during subsequent main initialization;
5. built-in shadowing, duplicates, invalid IDs, and late injection fail;
6. functional and opaque module values survive publication;
7. injection is isolated between pending handles;
8. no global or thread-local invocation state exists;
9. direct module loading cannot see invocation-local modules;
10. full workspace tests and warning-denied Clippy pass.

## Non-goals

- module mutation after initialization;
- injection by ordinary main code unless entry explicitly delegates the
  function and owning handle;
- filesystem/network convenience APIs beyond the initial Host observations;
- installation or process execution.

## Implementation result

`entry/rt.native.forma` is visible only to the Host-selected entry and offers
explicit-handle observations, injection, and initialization operations.

`inject_module` accepts a structural record witness and matching record value,
returning `True` on success. Each field becomes a module export scheme. Heap
publication preserves closures, native functions, opaque values, and identity;
no serialization is involved. Duplicate, built-in-shadowing, private,
relative, and late injections fail, and separate handles remain isolated.
