# ADR-0072: The SOM root classes follow IBM's layouts, read from the OS/2 Warp 4 Toolkit

- **Status:** Accepted; amends ADR-0070 (the `CClassData` layout, the stubs) and ADR-0059 (the layouts beyond an object's first word)
- **Date:** 2026-11-20
- **Sources:**
  - The code:
    - `src/loader/som.rs`: the release-order tables `SOMOBJECT_METHODS`, `SOMCLASS_METHODS` and `SOMCLASSMGR_METHODS`; `som_write_mtab`; the `somParentMtabStruct`s in `som_init`; `som_parent_resolve`, `som_find_method`, `som_raise`, `som_default_error`, `som_check_args`; the new `SOMClass` methods;
    - `src/loader/api_registry.rs`, `targets/os2api.def`, `src/loader/api_trace.rs`;
    - `samples/som_test/include/som.h` and the `som_test` `Makefile`.
  - Sample: `samples/som_layout_test`.
  - **The OS/2 Warp 4 Toolkit 4.5, an unofficial copy.**
    - **Where:** [bitwiseworks/os2tk45](https://github.com/bitwiseworks/os2tk45), commit `c0ec7250eded133e9152416a7ae8906c319f7dd6` (2018-09-04). The files are IBM-copyrighted.
    - **How it was used:** read only, as data, in a scratch directory. Nothing from it was built, run or copied into the tree, its binaries (the SOM compiler's emitters, `som.lib`) were not opened, and the copy was deleted after this stage.
    - **The files read** (SHA-256):

      | File | SHA-256 |
      |---|---|
      | `som/include/somobj.idl` | `be3f981c1cd5873f59e92ad9b0c5aef26188f48ee20a25df5010731241703452` |
      | `som/include/somcls.idl` | `a5454d79e498a84bb725f22b62ad3ae5e566eccbfe423f91b0caab14b521fc78` |
      | `som/include/somcm.idl` | `75f943af5a5f44da01f56beb08cad9f25bdaaa2abd9d2b3a69e463ca590c5263` |
      | `som/include/somapi.h` | `cfb1016c4172ea567479694c8e3074cb52507d9a41aa75cbfce43ab826ca51bc` |
      | `som/include/somcdev.h` | `d9640d158a8d747b33cb184e10bd5dcd8827e1270c306c4be92b89cb17b91967` |
      | `som/include/somobj.h` | `32dc9743824ae9b916ffddc33f42db8530eac920f6589f57127762bfd4141aa8` |
      | `som/include/somcls.h` | `1c0761e958e7628e99bdd0649bf2167633c17d43c7406400a559df56b844e473` |
      | `som/include/boa.xh` | `f75b0227cacb52c8fab9673a5cb584632fabb32177db0b48fbafb4f8aaa8a5aa` |

    - **A consistency check:** each `.idl` file's `releaseorder` matches the `ClassData` of the generated `.h` file entry for entry (32, 76 and 22 tokens).
  - IBM *SOM Programming Guide* ("SOM Kernel Error Codes") and *SOM Programming Reference*, as mirrored on komh.github.io: the error codes, `somParentResolve`, `somFindMethodOk`.

## Context

A program built with IBM's SOM compiler and headers reads more than an object's first word:
- **`ClassData`.** It calls a method through the token at a fixed offset in `<Class>ClassData`: the class object, then one token per method in the IDL's `releaseorder`.
- **`CClassData`.** It finds parent procedures through `<Class>CClassData.parentMtab`.
- **Method tables.** IBM's C++ constructors compare an object's first word with `parentMtab->mtab`. `SOM_TestOn` code passes every receiver through `somTestCls` and calls the method on what it returns.

Warpine's own layouts did not match:
- the root `ClassData` held 14 / 17 / 8 tokens in Warpine's order, where IBM has 32 / 76 / 22;
- method tables were `[classObject, count, entries…]`;
- `parentMtab` pointed straight at the parent's table (ADR-0070);
- `somTestCls`, `somTest`, `somAssert` and `somCheckArgs` were stubs returning 0.

The public reference pages describe the calls but not these layouts. The OS/2 Warp 4 Toolkit headers, which the project's policy allows as a source, do.

## Decision

### `ClassData`

- **IBM's release order.** The root classes' `ClassData` hold one token for every method of the release order, in that order: 32 for `SOMObject`, 76 for `SOMClass`, 22 for `SOMClassMgr`, the `somPrivate*` / `somcPrivate*` placeholders included.
- **Tokens.** They stay Warpine's opaque numbers. The runtime makes them in table order, so the tokens of `somInit` and the other methods it calls itself come from the tables (`root_token`).
- **The hand-written bindings follow.** `som.h` declares the same order, and `som_test`'s objects now rebuild when `som.h` or `som.xh` changes.
- **Methods Warpine lacks.** A method of the order that Warpine does not implement resolves to `WrpSomMethodNotImplemented`. It raises `SOMERROR_NotImplementedYet` with warning severity (20171) through `SOMError`, then returns 0. Today these are the copy, assign, dispatch, cast, reset and renew families, the private slots, and the class manager's interface-repository and class-library methods.

### Method tables

They have `somMethodTab`'s header, then the entries:
- `classObject`, `classInfo` (0), `className`, `instanceSize`, `dataAlignment` (4), `mtabSize` (bytes, header included), `protectedDataOffset`, `protectedDataToken`, `embeddedObjs` (0).

### `CClassData`

- **`parentMtab`** points to a `somParentMtabStruct` of 22 words:
  - the class's own method table;
  - `next`, the list of its parents' tables;
  - the class object;
  - the renew thunk (0);
  - the instance size;
  - the initializers, resolved tokens, three control blocks and counts (0).
- **`instanceDataToken`** follows, as in ADR-0070.
- **`SOMObjectCClassData`** also holds six `somDispatch*` procedures, which are the not-implemented procedure.

### Parent resolution

- **`somParentNumResolve(parentMtab, n, token)`** (SOM 2.x, imported by name, ordinal 3077) resolves in the `n`th parent, counting from 1, in the list after the class's own table. `somParentResolve` is `n` = 1.
- **The guest's list only names the table.** The runtime answers from its records of that table's class.

### New `SOMClass` methods of the release order

- **Versions.** `somCheckVersion(major, minor)`: the same major version and at least the minor one; 0.0 asks for nothing. `somGetVersionNumbers`.
  - The root classes are 1.5 (`SOMObject_MajorVersion` … in the Toolkit headers).
  - `somInitClass` records a class's version; a class without one accepts any.
- **Method lookup.** `somFindMethod` / `somFindMethodOk`, `somFindSMethod` / `somFindSMethodOk` and `somLookupMethod` look up a method by somId. The `…Ok` forms and `somLookupMethod` raise `SOMERROR_MethodNotFound` (20039, fatal) when it is missing.
- **The rest.** `somGetClassMtab`, `somGetInstanceOffset`, `somAllocate` / `somDeallocate`.

### Former stubs, as `somapi.h` / `somcdev.h` declare them

- **`somTestCls(obj, class, file, line)`** returns the object when it is one and an instance of the class or of a descendant. Otherwise it raises `SOMERROR_TestObjectFailure` (20109).
- **`somTest(condition, severity, file, line, msg)`** raises `SOMERROR_FailedTest` with the given severity (`20110 + severity`) when the condition fails.
- **`somAssert(condition, code, file, line, msg)`** raises `code` when the condition fails. A kept assertion writes "Assertion passed: msg" through `SOMOutCharRoutine` when `SOM_AssertLevel` is above 0.
- **`somCheckArgs(argc, argv)`** sets the levels from `-somt` / `-somtc` (trace 1 / 2), `-soma` / `-somac` (assert 1 / 2) and `-somw` (warn 1).

`somConstructClass` waits for the class-building stage. `somSaveMetrics` and `somWriteMetrics` stay stubs: no source describes them.

### Errors the kernel raises

- **A program's own `SOMError`** is called with the code, the file (`som.dll` when the kernel raises the error) and the line, and the call then returns its result.
- **The default `SOMError`** writes the message and ends the process on a fatal code.

### Warpine's choices (unverified)

- **The Toolkit copy.** It is unofficial and was not checked against an IBM distribution. The layouts rest on it, and on the public reference pages where those cover the same points.
- **Methods Warpine lacks** raising a warning rather than IBM's fatal `SOMERROR_NotImplementedYet`, so programs go on.
- **`somParentMtabStruct`.** Its fields after the instance size being 0, and parent numbering from 1 after the class's own table.
- **Versions.** A class without a version accepting any.
- **`somTestCls`'s error code**, and the text of a kept `somAssert`.
- **`somCheckArgs`'s levels**, and its handling of unknown flags (they are ignored).
- **`dataAlignment` 4**, and `classInfo` / protected data 0.

## Consequences

- **Programs built with IBM's SOM headers** read the root classes' `ClassData`, method tables and `CClassData` where they expect them. Calls to methods Warpine lacks no longer resolve silently.
- **Classes built by guests,** through our bindings, get IBM's method-table header. Their own `CClassData` are left to the class-building stage.
- **The entry-point count** grows from 769 to 782 (SOM: 112 implemented, 3 stubs).
- **Still open:**
  - `somBuildClass` with IBM's static class information, `somConstructClass`, the initializer and destructor control blocks (Stage B);
  - the methods resolving to the not-implemented procedure;
  - IBM's ordinals 48 and above.
- **Tests:**
  - `som.rs`:
    - `class_data_follows_ibm_s_release_order`;
    - `method_tables_have_ibm_s_header`;
    - `class_versions_lookup_and_allocation`;
    - `testing_entries_and_argument_flags`;
    - `new_class_procedures_and_parent_resolution`, reworked;
  - `samples/som_layout_test`: 25 checks;
  - `samples/som_test` and `samples/som_ordinal_test`, rebuilt against the new layouts with unchanged output.
