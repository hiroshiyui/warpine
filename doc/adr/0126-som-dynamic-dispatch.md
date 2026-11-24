# ADR-0126: SOM dynamic dispatch through apply stubs somidl writes

- **Status:** Accepted; extends ADR-0122 (somidl) and ADR-0072 (the root classes)
- **Date:** 2026-11-25
- **Sources:**
  - IBM's OS/2 Warp 4 Toolkit *System Object Model Programming Reference* (kept in `doc/references/`, cited only): `somApply`, `somDispatch` / `somClassDispatch` (`SOMObject`), `somVaBuf_create` / `_add` / `_get_valist` / `_destroy`, `somvalistGetTarget` / `SetTarget`, `TypeCode_kind`'s `TCKind` enumeration, `somMethodData`, `somAddStaticMethod`.
  - The OS/2 Toolkit 4.5's `somobj.idl` (public copy listed in the README) for `somDispatchV` / `L` / `A` / `D`, which the Reference only mentions ("the somDispatchX methods now invoke somDispatch").

## Context

`somDispatch` calls a method named at run time with its arguments in a `va_list` (the target object first, then each argument widened as ANSI C widens variadic arguments). The kernel can find the method's procedure by name, but not call it: the number and size of the arguments, and how the result comes back (a word, two words, the FPU, a structure), are known only to the method's bindings. SOM's answer is an *apply stub* per method, registered with `somAddStaticMethod`'s `applyStub` argument; `somApply` calls it, and returns FALSE when a method has none ("in support of old class binaries"). Warpine's kernel kept no such information, so `somDispatch`, `somClassDispatch` and `somDispatchV` / `L` / `A` / `D` were "not implemented", and `somApply`, the `somVaBuf_*` and `somvalist*Target` functions did not exist.

## Decision

1. **somidl writes an apply stub for every method** (`emit_c.rs`, `apply_stub`), in the implementation bindings: `static void SOMLINK somAP_<Class>_<method>(somToken *retVal, somMethodProc *method, char *args)`. It reads the target, the `Environment` (unless `callstyle=oidl`) and each argument from `args` (an `in` argument as its widened type — `short`, `char`, `boolean`, `octet` as `int`, `float` as `double` — then converted; `out` / `inout` ones as pointers; each `SOM_VA_SIZE` bytes), calls `method` through the method's `somTD_` type, and stores the result at `retVal` when it is not NULL. The compiler that builds the class therefore handles every return convention (words, `long long`, `float` / `double`, structures).
2. **The protocol is Warpine's own.** The stub takes a plain pointer to the arguments, not a `va_list`, because compilers pass `va_list` differently. It is registered as a `somApplyInfo` record (`SOM_APPLY_INFO_MAGIC` "WAPS", then the stub) in `somStaticMethod_t.applyStub` (and `somAddStaticMethod`'s argument, and layout 3's method table); the kernel uses only a record with the magic, for the class that introduces the method (`SomRuntime::apply`, by token). Anything else there, an IBM-built class's stub for instance, is ignored and the method is not applied.
3. **Root methods** (Warpine's thunks) need no stub: `somApply` calls the thunk with the registry's `argc` words from the arguments and stores its result word.
4. **va_lists.** GCC and clang pass a `va_list` as a pointer to the arguments; Open Watcom as a pointer to that pointer. The kernel takes whichever form leads to the target object (`somApply`'s `objPtr`; for `somDispatch` and `somvalist*Target`, a SOM object or a `somVaBuf`'s buffer). `somVaBuf_get_valist` writes the buffer's address to `*ap`, which is a valid `va_list` in both forms.
5. **The calls** (SOM.DLL ordinals unknown, so Warpine's own, ADR-0123): `somApply` 3102, `somVaBuf_create` … `somvalistSetTarget` 3103–3108; `SOMObject`'s `somDispatch` 3224, `somClassDispatch` 3225, `somDispatchV` / `L` / `A` 3226–3228.
   - `somDispatch(retValue, methodId, ap)` resolves `methodId` by name on the receiver's class, `somClassDispatch` on `clsObj`'s, and applies it; FALSE for an unknown method or one without an apply stub.
   - `somDispatchV` / `L` / `A(methodId, descriptor, ap)` call the object's own `somDispatch` (so an override takes effect, as the Reference says) and return its result word.
   - `somVaBuf_add` widens `tk_short`, `tk_ushort`, `tk_long`, `tk_ulong`, `tk_float`, `tk_double`, `tk_char`, `tk_boolean`, `tk_octet`, `tk_TypeCode`, `tk_enum`, `tk_string` and `tk_pointer` (the Reference's list); a buffer grows to at most 64 KB.
6. **The SDK** (`somapi.h`, `som.h`): `TCKind` (the Reference's enumerators in its order, so C's values from 0), `somVaBuf`, `somApplyStub`, `somApplyInfo`, `SOM_VA_SIZE`, the functions; the `somDispatch*` method types and macros, and the varargs `_somDispatch` / `_somClassDispatch` the Reference's example uses.

## Warpine's choices (unverified)

- The apply-stub protocol and `somApplyInfo` record (IBM's binding layout for apply stubs is not public); a class built with IBM's tools is not applied.
- `TCKind`'s values (the Reference lists the enumerators without values).
- `tk_char` widened as a signed `char`; `somVaBuf_add`'s result (1 / 0; the Reference gives none).
- Telling the two `va_list` forms apart by what they lead to.
- `somDispatchD` stays not implemented: its `double` comes back in the FPU, which a host call cannot set.
- `somAddDynamicMethod` and dynamic methods stay unsupported.

## Consequences

- Classes built with somidl's bindings can be called by name from a `va_list` or a `somVaBuf`; the Reference's examples run.
- Each class's implementation bindings grow by one small function and record per method.
- **Tests:** `som_dispatch.rs` (widening, both `va_list` forms, `somVaBuf` on the mock); `somidl.rs` (`implementation_bindings`, `apply_stubs_read_widened_arguments`); `samples/som_idl_test` (49 checks: `_somDispatch` varargs and va_list forms, `somDispatchL`, `somApply` with a stub and on a root method, `somClassDispatch`, `somvalist*Target`, a `short` / `float` / `double` method with a `double` result, an unknown method), built by GCC and clang.
