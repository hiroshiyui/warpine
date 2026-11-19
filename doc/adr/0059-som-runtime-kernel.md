# ADR-0059: A built-in SOM.DLL keeps classes host-side and objects in guest memory; executables load and initialize their DLLs

- **Status:** Accepted (stage 1: the SOM kernel's root classes, class building, dispatch and instance data); the token encoding and single inheritance are superseded by [ADR-0060](0060-som-metaclasses-multiple-inheritance.md); a failing `_DLL_InitTerm` at process start stops the program since [ADR-0063](0063-module-lifecycle.md)
- **Date:** 2026-11-19
- **Sources:**
  - The runtime: `src/loader/som.rs` (`SomRuntime`, `SomClass`, `som_call`, `som_init`, `som_data_address`), `src/loader/descriptors.rs` (data exports), `src/loader/pm_hook.rs` (`ChainThen::SomFree`)
  - Loading: `src/lx.rs` (the import procedure name table, `module_name_of`), `src/loader/lx_loader.rs` (`load_dependencies`, additive import fixups), `src/loader/managers.rs` (`pending_init`), `src/loader/warpine_api.rs` (`dll_init_chain`, `WARPINE.6`), `src/loader/stubs.rs` (`DosLoadModule`)
  - Sample `samples/som_test`: hand-written bindings (`include/som.h`, `hello/`, `classes/`) over the unchanged C sources of the Open Watcom SOM samples (`vendor/watcom/samples/os2/som`)
  - IBM *SOM Programming Guide* and *SOM Programming Reference* (SOMObject, SOMClass, SOMClassMgr, `somResolve`, `somClassResolve`, `somDataResolve`, `somIdFromString`, `somEnvironmentNew`), as on EDM/2 and the OS/2 Toolkit mirrors
  - Earlier decisions: ADR-0026 (call chains), ADR-0043 (structures in guest memory)

## Context

The System Object Model is the base of the Workplace Shell and of programs written for it. A SOM program never calls a method directly:
- it asks the runtime for the procedure by a token kept in the class's `ClassData`;
- every object begins with a pointer to its class's method table;
- class objects are themselves objects, instances of a metaclass.

Classes live in DLLs that export their `ClassData` and a `<Class>NewClass` function. A program imports both, and the runtime's own data (`SOMObjectClassData`, `SOMClassMgrObject`), from `SOM.DLL`.

Three constraints shaped the work:

- **Clean room.** IBM's `som.dll`, its SOM compiler (`sc`) and the DLLs `sc` needs are off limits (clean-room policy), and the user chose not to use somFree. The test programs are Open Watcom's SOM samples, their C sources unchanged. The bindings `sc` would generate from their `.idl` / `.csc` files are written by hand from the documented conventions.
- **Loader gaps.** Running them exposed four gaps in Warpine's LX loading:
  - imports by name never resolved: the parser bounded the import procedure name table by the loader section, though it ends the fixup section;
  - an import with an additive value (`ClassData.field`) lost the offset;
  - an executable never loaded the DLLs it imports;
  - only the module `DosLoadModule` named had its `_DLL_InitTerm` run, not those it imports.
- **Module names.** Open Watcom's import libraries name a DLL linked as `name x.dll` `X.DLL`.

## Decision

### SOM.DLL

- **A built-in module.** `SOM` gets its ordinals at `SOM_BASE` (22528). Its functions are API thunks:
  - `somEnvironmentNew`, `somResolve`, `somResolveByName`, `somClassResolve`, `somDataResolve`;
  - `somGetGlobalEnvironment`, `somIdFromString`, `somStringFromId`, `somCompareIds`, `somIsObj`.
- **Data exports.** `SOMObjectClassData`, `SOMClassClassData`, `SOMClassMgrClassData` and `SOMClassMgrObject` resolve to addresses in a block the runtime fills, not to thunks (`resolve_import`).
- **Root-class methods.** The methods of `SOMObject`, `SOMClass` and `SOMClassMgr` are thunks of the module too, so a method table holds thunk addresses for the root's methods and guest addresses for a class's own.
- **What the guest sees.**
  - Method tables `[classObject, count, entries…]`.
  - Class objects of one word: the metaclass's method table.
  - The `ClassData` structures: the class object, then the tokens of the class's own methods.
- **What Warpine keeps.** Each class's record (`SomClass`): name, parent, metaclass, methods by name inherited first, instance layout. Method lookups answer from these records, not from guest memory, so a program that overwrites a method table cannot steer `somResolve`.
- **Tokens.** A method token is its index in the class's methods plus one: inherited methods keep their indices, so a parent's token works on descendants. A data token is the offset of the class's instance data in the object. (Superseded by ADR-0060: tokens name their introducing class.)
- **Building classes** uses the documented `SOMClass` methods, as SOM 1.x bindings do: `somNew` on the metaclass, `somInitClass`, `somAddStaticMethod`, `somOverrideSMethod`, `somClassReady`.
- **Guest overrides run.** `somNew` and `somFree` call the object's `somInit` / `somUninit` — guest overrides included — through call chains (ADR-0026); `somFree` frees the memory after `somUninit` returns.
- **Bounds.** Classes, methods per class, live objects, `somId`s, instance data and name lengths are bounded (`MAX_*` in `som.rs`). `somFree` frees only objects `somNew` made.

### Loading

- **Import names.** The import procedure name table is read up to the end of the fixup section (at most 1 MB).
- **Additive imports.** An import fixup adds its additive value.
- **Module names.** An imported module name ending in `.DLL` stands for the module without it.
- **Executables load their DLLs.** `Loader::load` loads the DLLs a program imports, recursively, before its fixups (`load_dependencies`, shared with `load_dll_impl`). Every module loaded with an entry point goes on `DllManager::pending_init` after the modules it imports.
- **Initialization at process start.** When modules are waiting, the program starts in `WARPINE.6` (`WrpStartProcess`). It calls their `_DLL_InitTerm(hmod, 0)` as one call chain and returns into the program's entry point, with the entry stack untouched.
- **`DosLoadModule`** runs the routines of the modules loaded with the one named, then its own, the same way. The handle is written only when every routine succeeded, else `ERROR_INIT_ROUTINE_FAILED`.

### Warpine's choices (unverified against IBM's SOM)

- **Layouts and tokens.** The layouts beyond an object's first word, the token encoding and the order of the root classes' `ClassData` fields are Warpine's own.
- **Ordinals.** The module's ordinals are Warpine's; programs import by name. Programs built against IBM's `som.lib` import by IBM's ordinals, which are not known yet.
- **Inheritance.** Only single inheritance; `somInitClass`'s instance size is the class's own data. (Superseded by ADR-0060: multiple inheritance, derived metaclasses.)
- **Method tracing** (`SOM_TraceLevel`) is not kept.
- **Failed initialization at start.** A `_DLL_InitTerm` failing at process start does not stop the program; OS/2 refuses to load it.

## Consequences

- **What works.** `samples/som_test` builds Open Watcom's `helloc` and `classes` samples unchanged against the hand-written bindings, and both run correctly:
  - a class with one method;
  - four classes in four implicitly loaded DLLs, with inheritance, overrides, parent calls, instance data and `ClassData` imported across DLLs.
- **Loader fixes for every program.** By-name imports, additive imports, implicitly loaded DLLs and their initialization now work for all LX programs.
- **Still missing** (stage 2 onwards):
  - metaclasses with their own instance data and methods, attributes, sequences and `somDumpSelf` (the `animals` sample), multiple inheritance, `SOMMalloc` and the other runtime variables — done in ADR-0060;
  - the C++ bindings (`hellocpp`);
  - `somFindClass` loading a class's DLL;
  - IBM's ordinals;
  - the rest of `SOMClassMgr`, `SOMU`, the interface repository, DSOM;
  - the Workplace Shell classes.
