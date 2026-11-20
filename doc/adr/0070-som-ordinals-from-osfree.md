# ADR-0070: SOM.DLL's ordinals 1–47 come from osFree's export list; Warpine's own move to 512–1023

- **Status:** Accepted; amends ADR-0059 (the SOM runtime kernel), on its ordinals. Amended by ADR-0071: the SOM module is 4096 ordinals wide, IBM's later exports have 48–3071, and Warpine's own moved to 3072–4095. Amended by ADR-0072: `CClassData.parentMtab` is a `somParentMtabStruct` (the class's own table first), and `somAssert`, `somTest`, `somTestCls` and `somCheckArgs` are no longer stubs
- **Date:** 2026-11-20
- **Sources:**
  - The code:
    - `src/loader/som.rs` (the ordinal constants, `data_export_size`, `data_offsets`, the new arms of `som_call`, `som_parent_resolve`, `som_output`, `som_prefix`);
    - `src/loader/api_registry.rs` (the SOM entries, `STUB_ORDINALS`);
    - `targets/os2api.def`, `src/loader/api_trace.rs`.
  - Sample: `samples/som_ordinal_test`.
  - **osFree**: the `EXPORTS` of `SOM/som/makefile` in [osfree-project/osfree](https://github.com/osfree-project/osfree), `master` branch. The repository is BSD-3-Clause; the file has no header of its own. Only this list of names and numbers is used. osFree's SOM headers, IDL and kernel come from somFree, which the project does not use.
  - IBM *SOM Programming Reference* and *SOM Programming Guide* (OS/2 Warp 4 Toolkit, as mirrored on EDM/2 and komh.github.io): `somCheckId`, `somRegisterId`, `somUniqueKey`, `somTotalRegIds`, `somSetExpectedIds`, `somBeginPersistentIds`, `somEndPersistentIds`, `somPrefixLevel`, `somLPrintf`, `somVprintf`, `somParentResolve`, `SOMClassInitFuncName`, "Using `<className>NewClass`", the `SOM_Test` and `SOM_Assert` macros.

## Context

ADR-0059 gave `SOM.DLL` Warpine's own ordinals, and programs imported it by name. A program linked against IBM's `som.lib` imports it by IBM's numbers instead. Real Workplace Shell classes and SOM programs built with IBM's toolkit are such programs, so they could not load.

Open Watcom ships no SOM import library. osFree's SOM makefile exports 47 names with ordinals: the SOM 1.0 set. Its numbers match Free Pascal's `som.pas`, an independent list, which makes them very likely IBM's. Nothing public lists ordinals 48 and above except Free Pascal's LGPL `som.pas`, which the project chose not to use.

Warpine's own exports (the `Wrp*` functions and the root classes' method thunks) sat at numbers IBM uses. A built-in module has 1024 ordinals (`SOM_BASE + 1024 <= STUB_AREA_SIZE`).

## Decision

### Numbering

- **1–19: data**, in osFree's order. These are the root classes' `ClassData` and `CClassData`, `SOMClassMgrObject`, and the runtime variables, `SOMClassInitFuncName` among them. The reference declares it a pointer to a function returning the name of a class library's initialization function.
- **20–47: functions**, in osFree's order.
- **48–511: left unassigned** for IBM's later exports. The SOM 2.x functions Warpine already has (`somResolveByName`, `somClassResolve`, `somGetGlobalEnvironment`, `somIsObj`, `somSetOutChar`) stay reachable by name only, until their numbers are known.
- **512–1023: Warpine's own exports.**
  - those five SOM 2.x functions at 512–516;
  - the procedures the runtime variables hold at 520–529;
  - `WrpSomMethodError` at 599;
  - the methods of `SOMObject`, `SOMClass` and `SOMClassMgr` from 600, 700 and 800.
- **Checked by the build and the tests.** A compile-time assertion keeps the private range at 512 or above. A unit test checks the 47 names against the osFree table, and checks that no other SOM export falls below 512.

### Data exports from a table

The data block is laid out by ordinal (`data_export_size`, `data_offsets`). A data import by ordinal or by name resolves to its address, a function to its thunk.

### `<Class>CClassData`

It has two words:
- **`parentMtab`**, the documented field. It points to a list of `{method table, next}` entries, which for `SOMClass` and `SOMClassMgr` holds `SOMObject`'s table; `SOMObject` has none.
- **The class's instance-data token** (Warpine's choice).

### The 25 new entries

- **Implemented from the references:**
  - **`<Class>NewClass(major, minor)`** for the three root classes returns the class object, accepting every version.
  - **`somParentResolve(parentMtab, token)`** resolves through the class the first table belongs to, using the runtime's records, so a guest's table contents cannot steer it.
  - **The id calls.** `somCheckId` and `somRegisterId` register an id under its string; `somRegisterId` returns 1 for a new one. `somUniqueKey` gives one key per string; `somTotalRegIds` counts the ids.
  - **The no-effect calls.** `somSetExpectedIds`, `somBeginPersistentIds` and `somEndPersistentIds` do nothing.
  - **The printing calls.** `somPrefixLevel` writes blanks; `somLPrintf` writes them, then formats; `somVprintf` takes a `va_list`. All write through `SOMOutCharRoutine`, as `somPrintf` does.
  - **`SOMClassInitFuncName`** holds a procedure that returns "SOMInitModule".
- **Stubs returning 0:** `somAssert`, `somTest`, `somTestCls`, `somCheckArgs`, `somConstructClass`, `somSaveMetrics`, `somWriteMetrics`. The references describe only the `SOM_Test` and `SOM_Assert` macros, not these functions. They count as stubs in `--compat`.

### Attribution

osFree is credited in the README's sources, in this record, and in source comments in `som.rs`, `api_registry.rs`, `os2api.def` and the sample.

### Warpine's choices (unverified)

- **The ordinals.** osFree's numbers are IBM's. Free Pascal's list agrees, but nothing has been checked against IBM's `som.lib`.
- **The entries' shapes.** Ordinals 1–19 being data and 20–47 functions, as the names and the reference suggest.
- **`CClassData`.** The instance-data token as its second word.
- **`<Class>NewClass`.** Accepting every version for the root classes.
- **The printing.** Two blanks per `somPrefixLevel` level, capped at 64 levels.
- **The id calls.** `somUniqueKey` returning the runtime's id for the string.
- **The stubs.** The seven undocumented entries doing nothing; their arguments are unknown.
- **`SOMClassInitFuncName`.** Class loading still asks for "SOMInitModule" when a program has replaced the variable.

## Consequences

- **IBM-linked programs.** A program linked against IBM's `som.lib` that uses only SOM 1.0 entries now loads, and its imports reach the right entries. An import of 48 or above still fails at load with an unknown ordinal.
- **Existing programs.** Programs that import by name, such as Open Watcom's SOM samples and Warpine's own bindings, are unchanged.
- **The entry-point count.** It grows from 743 to 769.
- **Still open:**
  - IBM's ordinals 48 and above;
  - the `ClassData` and method-table layouts from the Toolkit's IDL `releaseorder`, a later stage with its own record;
  - a replaced `SOMClassInitFuncName` being called by class loading.
- **Tests:**
  - `som.rs`:
    - `ordinals_follow_osfree_and_keep_ibm_s_range_free`;
    - `data_exports_are_the_first_nineteen`;
    - `new_class_procedures_and_parent_resolution`;
    - `ids_are_registered_and_counted`;
    - `prefixed_and_va_list_printing`;
  - `samples/som_ordinal_test`: 26 checks of imports by number;
  - `test_som_samples_run_on_the_som_runtime`: the by-name samples, unchanged.
