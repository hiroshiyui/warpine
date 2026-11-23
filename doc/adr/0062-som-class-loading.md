# ADR-0062: SOM loads a class's DLL by name through replaceable runtime variables; DLLs are searched by plain module name only

- **Status:** Accepted (SOM stage 4: class loading, the replaceable runtime variables)
- **Date:** 2026-11-19
- **Sources:**
  - The runtime: `src/loader/som.rs` (`som_find_class`, `som_module_steps`, `som_class_loaded`, `som_module_loaded`, `som_printf`, the default `SOMLoadModule` / `SOMDeleteModule` / `SOMError` / `SOMOutCharRoutine`)
  - Loading: `src/lx.rs` (`resident_names`, `read_name_table`), `src/loader/lx_loader.rs` (exports by name, `is_plain_module_name`)
  - Sample `samples/som_test/findcls` (Warpine's own: `greeter.dll`, `echoes.dll`, `findcls.exe`)
  - IBM *SOM Programming Guide* ("Customizing class loading and unloading", "Customizing character output", "Customizing error handling") and *SOM Programming Reference* (`somFindClass`, `somFindClsInFile`, `somLocateClassFile`, `somLoadClassFile`, `SOMLoadModule`, `SOMDeleteModule`, `SOMError`, `SOMOutCharRoutine`, `somSetOutChar`, `SOMInitModule`), as on EDM/2
  - Earlier decisions: ADR-0059 (the SOM kernel, DLL loading), ADR-0060, ADR-0061, ADR-0026 (call chains)

## Context

A SOM program can use a class it was not linked with. It asks the class manager for the class by name (`somFindClass`); if the class does not exist yet, SOM loads the DLL that contains it and runs the DLL's initialization function, which creates the class. The Workplace Shell creates its classes this way: their DLLs are registered by name, never imported.

SOM also lets a program replace parts of the runtime through variables:
- `SOMLoadModule` and `SOMDeleteModule` load and unload class DLLs;
- `SOMError` handles errors;
- `SOMOutCharRoutine` receives `somPrintf`'s output, character by character.

Running a DLL of Open Watcom's showed two loader gaps:
- **Exports by name.** Open Watcom's linker names a DLL's exports in the *resident* names table, and Warpine only read the non-resident one. So no export of such a DLL could be found by name, not by `DosQueryProcAddr` either.
- **Host paths.** `find_dll_path` put the guest's module name straight into a host path. A name such as `../../x` reached outside the program's directory, through `DosLoadModule` too.

## Decision

### Finding and loading classes

- **`somFindClass(mgr, id, major, minor)`** returns the class if it exists. Otherwise it loads the class's file, whose name `somLocateClassFile` gives.
  - **`somLocateClassFile`** returns the class's name: without an interface repository there is no `dllname` to read.
  - **`somFindClsInFile`** names the file itself.
  - **`somLoadClassFile`** loads the file even when the class exists.
- **The loading goes through the variable `SOMLoadModule`,** called with:
  - the class name, the file, `"SOMInitModule"`, the versions, and a cell for the module handle;
  - 0 is success.

  Afterwards the class is looked up by name: no class, or a failed load, gives 0.
- **The default `SOMLoadModule`:**
  1. loads the DLL through the ordinary loader;
  2. runs the `_DLL_InitTerm` of the DLL and of the modules loaded with it;
  3. calls the DLL's initialization function `(major, minor, className)`, or, when the DLL lacks one, `<className>NewClass(major, minor)`;
  4. writes the module handle.

  A failing `_DLL_InitTerm` makes it return `ERROR_INIT_ROUTINE_FAILED`, and a missing file `ERROR_MOD_NOT_FOUND`.
- **The default `SOMDeleteModule`** is `DosFreeModule`.
- **Guest calls are call chains (ADR-0026).** When `SOMLoadModule` still holds the default, `somFindClass` makes the default's calls itself in one chain. Otherwise it calls the program's procedure, which may call the default in turn as a nested chain.

### The other runtime variables

- **Exported data** (ordinals 31–34): `SOMOutCharRoutine`, `SOMLoadModule`, `SOMDeleteModule` and `SOMError`, holding the runtime's own procedures at first.
- **`somPrintf` writes through `SOMOutCharRoutine`.** While it holds the default, the text is written at once. Otherwise a chain calls the program's routine for every character, at most 64 KB.
- **`somSetOutChar(routine)`** sets the variable; `somSetOutChar(0)` puts the default back.
- **The default `SOMError(code, file, line)`** writes `SOM error <code> in file <file>, line <line>` to standard error. When the code's last digit is 9 (`SOM_Fatal`), it ends the process with the code as its result.

### Loading fixes for every program

- **Resident names.** `LxFile::resident_names` reads the resident names table, the module's own name (ordinal 0) left out. A DLL's exports by name come from both tables.
- **Bounded tables.** Both tables are read by `read_name_table`, bounded by the table's size and 1 MB at most.
- **Plain module names only.** `find_dll_path` searches only plain module names: none with `/`, `\`, `:`, `.` or `..` as the whole name, or control characters, and at most 255 bytes. That covers `DosLoadModule`, implicit imports and SOM's class files alike.

### Warpine's choices (unverified against IBM's SOM)

- **Names and ordinals.**
  - The ordinals of the new functions, variables and `SOMClassMgr` methods.
  - The order of the new `ClassData` fields: `somFindClsInFile`, `somLocateClassFile` and `somLoadClassFile` after `somRegisterClass`.
- **Loading.**
  - `somLocateClassFile` returns the class name.
  - `somLoadClassFile` loads even an existing class.
  - The fallback to `<className>NewClass`.
- **Defaults.**
  - The default `SOMError`'s message and exit code.
  - `somSetOutChar(0)` restoring the default.
- **What is not routed.** `somPrintSelf` / `somDumpSelf` still write directly, not through `SOMOutCharRoutine`.
- **Unloading.** Unloading a module through `SOMDeleteModule` does not remove its classes; using them afterwards is undefined, as in SOM.

## Consequences

- **Classes by name.** A program can now use classes from DLLs it never imports, by name: the way the Workplace Shell loads its classes.
- **Exports by name.** `DosQueryProcAddr` and SOM find exports by name in Open Watcom's DLLs.
- **A closed hole.** A guest can no longer make Warpine load a file outside the program's directory and `C:\OS2\DLL` by giving a path as a module name.
- **The check.** `findcls.exe` exercises:
  - `somFindClass` with `SOMInitModule`;
  - `somFindClsInFile` through the program's own `SOMLoadModule`, ending in `EchoNewClass`;
  - a missing class, and `somLocateClassFile`;
  - `SOMDeleteModule`;
  - `somSetOutChar`;
  - a program's `SOMError`, then the default ending the program.
- **Still missing:**
  - `somUnloadClassFile` and removing a module's classes;
  - the interface repository's `dllname`;
  - `somBuildClass`;
  - IBM's ordinals and layouts.

## Addendum (2026-11-23): versions, the initialization function's name, and a DLL without an entry point

The same check against the Reference corrected class loading:

- **Versions.** `somFindClass`, `somFindClsInFile` and `somLoadClassFile` return NULL when the class, registered or just loaded, does not meet the version asked for ("a NULL pointer is returned if the class's version is not compatible"). The rule is `somCheckVersion`'s: the same major version and at least the minor one, 0.0 asking for nothing.
- **`SOMClassInitFuncName`.** The name of the initialization function passed to `SOMLoadModule` is the one the procedure in `SOMClassInitFuncName` returns (`somGetInitFunction`). Warpine's default still returns "SOMInitModule"; a program's replacement is called on the program's thread before the module is loaded, and an empty or unreadable name falls back to "SOMInitModule" (Warpine's choice).
- **No entry point.** The default `SOMLoadModule` fails when the DLL has neither the initialization function nor `<className>NewClass`. Which error it returns, `ERROR_PROC_NOT_FOUND`, is Warpine's choice; the DLL stays loaded, as after a failed initialization function.
