# ADR-0122: somidl, a SOM IDL compiler for the SDK, with SOM headers and an initializer protocol of Warpine's own

- **Status:** Accepted 2026-11-23 (builds on ADR-0059–0063 and ADR-0070–0073; follows the SDK's own compilers, ADR-0099 `os2rc` and ADR-0105 `os2msg`)
- **Date:** 2026-11-23
- **Sources:**
  - **What SOM IDL is:**
    - IBM's OS/2 Warp 4 Toolkit *System Object Model Programming Guide* ("SOM IDL and the SOM Compiler": the language, the `implementation` section, modifiers, `releaseorder`, the emitters and the files they write);
    - IBM's *SOMobjects Developer's Toolkit* Version 3.0 *Programmer's Guide*, Vol. I (1996), on the same, at version 3.0;
    - both on the Internet Archive (README, "Sources"), kept locally and only cited.
  - **The base language:** the OMG's *CORBA 2.0* IDL chapter (public; SOM IDL is CORBA IDL with SOM's extensions).
  - **What the kernel expects:**
    - the OS/2 *System Object Model Programming Reference* (`somBuildClass`, the `SOMObject` / `SOMClass` / `SOMClassMgr` methods, the runtime functions);
    - the layouts already taken from the Toolkit headers (ADR-0072, ADR-0073).
  - **Test inputs:** the `.idl` files of Open Watcom's SOM samples (`vendor/watcom/samples/os2/som`), read in place as inputs, never copied.
  - **Not used:** IBM's SOM compiler `sc` and its emitter DLLs (IBM binaries); the Toolkit's emitter templates (`*.efw`) and generated headers; somFree (excluded by decision).

## Context

Warpine's SOM kernel runs programs that make their classes through the documented `SOMClass` methods (hand-written bindings, `samples/som_test`) or through `somBuildClass` (ADR-0073). Programs built with IBM's toolkit need more than Warpine can give them:
- they import `SOM.DLL` by IBM's ordinals, and those above 47 are unknown (ADR-0070, ADR-0071);
- their generated bindings expand `BeginInitializer`, `Init_<Ancestor>_…` and `EndDestructor` into code that uses the initializer and destructor control blocks, whose layout and indexing no public source describes. The OS/2 Reference and the SOMobjects 3.0 Guide give only the semantics (Stage B2/B3, on hold since 2026-11-20).

Real SOM code is not written as bindings by hand. Its classes are declared in SOM IDL, and IBM's `sc` writes the usage bindings (`.h`, `.xh`) and implementation bindings (`.ih`, `.xih`) that programs and class libraries compile against. Warpine's SDK has no SOM headers and no IDL compiler, so a program can only use SOM through hand-written declarations.

A program rebuilt from source with bindings Warpine generates does not depend on either gap:
- it imports `SOM.DLL` by name;
- its initializer code is whatever Warpine's emitter writes, so it only has to agree with Warpine's own kernel.

## Decision

1. **`somidl` (`src/bin/somidl.rs`, the compiler in `src/somidl/`)** compiles SOM IDL into C and C++ bindings, as `os2rc` and `os2msg` do for their formats:

   ```bash
   somidl [-I dir]... [-D name[=value]]... [-s emitters] [-o dir] file.idl
   ```

   Emitters (`-s`, default `h,ih`):
   - `h` / `ih`: C usage and implementation bindings;
   - `xh` / `xih`: the same for C++;
   - `c` / `xc`: an implementation template, written only when the file does not exist yet;
   - `def`: the class library's exports for `lx-link --export`.

   The file names and the C names follow the SOM Programming Guide's description, so source written for SOM compiles unchanged where it uses only documented names: `<Class>NewClass`, `<Class>ClassData`, `<Class>CClassData`, `_<method>` macros, `<Class>MethodDebug`, `somSelf`, `somThis`, `<Class>GetData`, `SOM_Scope`, `SOMLINK`.
2. **The language:** CORBA 2.0 IDL with SOM's extensions, as the Guide describes them:
   - `interface`, `module`, attributes, operations with `in` / `out` / `inout`, `oneway`, `raises`, `context`;
   - types: basic types, `struct`, `union`, `enum`, `sequence`, `string`, arrays, `typedef`, `const`, `exception`, `any`;
   - the `implementation` section: `releaseorder`, `metaclass`, `majorversion` / `minorversion`, `dllname`, `filestem`, `functionprefix`, `callstyle`, `directinitclasses`, `somallocate` / `somdeallocate`, instance variables, `passthru`;
   - per-method modifiers: `override`, `init`, `procedure`, `noself`, `nonstatic`, `namelookup`.
   - The C preprocessor is the one `os2rc` already has (`#include`, `#define`, `#ifdef`, `__SOMIDL__` defined).
   - SOM 1.0's OIDL (`.csc`) is not accepted at first.
3. **The SDK's SOM headers and IDL, written for Warpine from the Reference:**
   - `som.h` / `som.xh`, `somapi.h`, `somcdev.h` (the binding macros);
   - `somobj.idl`, `somcls.idl`, `somcm.idl` for the root classes, in IBM's release orders (ADR-0072), so the generated `ClassData` layouts match the kernel's.
   - Nothing is copied from IBM's headers; their names follow the Reference.
   - `check-headers` covers them (every declared call in `os2api.def`, ADR-0096).
4. **Class creation:** a generated `<Class>NewClass` builds the class through `somBuildClass` with a `somStaticClassInfo` (ADR-0073), the documented path. This exercises the kernel as IBM-built libraries will, and keeps the bindings free of kernel internals.
5. **An initializer and destructor protocol of Warpine's own** (closing the semantics part of Stage B2/B3 for rebuilt code):
   - `somInitCtrl` / `somDestructCtrl` / `somAssignCtrl` are Warpine's structures, declared in the SDK's `somapi.h`: a mask with one flag per class in the visit order, and a pointer into a per-class table of the ancestors' initializer procedures and data offsets.
   - The kernel builds each class's table when the class becomes ready: the transitive closure of `directinitclasses` (default: the parents), left to right, each class once, as the Guide's semantics require. A null `ctrl` is answered by that table.
   - `BeginInitializer`, `Init_<Ancestor>_<init>`, `BeginDestructor`, `DeInit_<Ancestor>_somDestruct` and `EndDestructor` are generated macros that walk the table.
   - The design is recorded as Warpine's own and unverified. It is not IBM's binary protocol: an IBM-built class library still cannot use it, which ADR-0073's Stage B2/B3 continues to track.
6. **Stages:**
   - **S1:** the parser and the C emitters for single inheritance, static methods, instance data, attributes and `somDefaultInit` / `somDestruct` with the new protocol. Rebuild `samples/som_test`'s classes from IDL (Open Watcom and GCC / clang).
   - **S2:** the C++ emitters (`xh`, `xih`), the SDK's C++ runtime (ADR-0103).
   - **S3:** multiple inheritance and `directinitclasses`, user-defined initializers (`init`), metaclasses, the copy and assign initializers.
   - **S4:** exceptions and `Environment` (`raises`, which needs the kernel's `somSetException` family), sequences, `any`, TypeCodes.
   - **Later, if needed:** OIDL `.csc`, the `ir` emitter and an interface repository, DSOM.
7. **Safety:** `somidl` parses untrusted files, so it gets a fuzz target (`fuzz/`, ADR-0112) with bounded nesting, include depth and sizes, as `os2rc` does. It runs on the host only and writes only the files its arguments name.

## Alternatives considered

- **Keep hand-written bindings** (today's `samples/som_test/include/som.h`): enough for tests, but every class needs pages of declarations, and no existing SOM source compiles against them.
- **Use IBM's `sc` and emitters:** they are IBM binaries, excluded by the clean-room policy, and their output would need IBM's unknown initializer protocol.
- **Use somFree's compiler:** excluded by decision.
- **Bindings that call `somInitClass` / `somAddStaticMethod` instead of `somBuildClass`:** simpler, but they would leave `somBuildClass`, the path IBM-built libraries take, without a second user, and make generated code differ more from what SOM programmers know.
- **Drop SOM for XPCOM** (`doc/TODOs.md`, "Object model options"): a separate decision. This proposal assumes the current course, SOM.

## Consequences

- SOM source that uses only documented interfaces can be rebuilt with GCC, clang or Open Watcom and run on Warpine's kernel, with no dependency on IBM's ordinals or on IBM's unpublished initializer layout.
- Warpine's own SOM classes, and later a Workplace Shell of Warpine's own, can be written in IDL rather than as hand-written bindings.
- The kernel's missing entries become needed in the order the stages use them: environments and exceptions (S4), the copy / assign initializers (S3), dispatch for `namelookup` and dynamic methods (later).
- Programs built with `somidl` run only on Warpine. They import `SOM.DLL` by name, and their initializers use Warpine's protocol, so they would not run on IBM's SOM. This matches the "API-compatible, not a clone" goal.
- Source that relies on undocumented details of IBM's generated bindings (macro internals, the layout of the control blocks, private `somc*` methods) will not compile, or will behave differently. Such cases are handled case by case.
- A new tool to maintain: a parser, four emitters, SDK headers and a fuzz target. Its output has no oracle, since `sc` cannot be run. Tests check the generated code by compiling and running it against the kernel, and against the Guide's descriptions of the bindings.

## Unverified (Warpine's choices, recorded in `doc/unverified.md`)

- The control-block structures, their table, and the macro names beyond those the Guide shows.
- The exact text and formatting of generated files, the implementation template's contents, and the `def` emitter's output.
- Which SOM IDL modifiers are accepted, ignored or refused in S1.
- `__SOMIDL__` and any other predefined macros.

## Addendum (2026-11-23): batches 1 and 2, the shared preprocessor and the front end

- **The preprocessor** moved from `src/bin/os2rc.rs` to `src/cpp.rs`, which `os2rc` and `somidl` include with `#[path]` (as `msgfile.rs` is shared). What differs between the tools is configuration: the predefined names, whether an included file emits its lines (`os2rc`: only `.rc` / `.dlg`; `somidl`: every file) and a statement that includes a file (`os2rc`'s `RCINCLUDE`). The text one run reads is now bounded (16 MB, includes counted) besides the include and expansion depths. `os2rc`'s output stays byte-identical to `wrc -zm`.
- **The front end** (`src/somidl/`: `ast.rs`, `parser.rs`, `dump.rs`) parses the whole of stage S1's language and, so that Open Watcom's `animals.idl` parses, also `sequence`, `any`, `raises` and `context`, which the emitters will use from stage S4. `union` and `exception` are refused with "not supported yet". Constant expressions see the constants declared before them, a scoped name by its last part. Modules, structs and sequences nest at most 32 deep.
- **The implementation section** is parsed by shape, not by a list of known modifiers: `name = value` and a bare `name` are class modifiers, `member: modifier[, modifier = value]` are member modifiers, anything else is an instance variable. Which modifiers mean something is the emitters' business (batch 5).
- **`--dump`** prints the tree as indented text, one declaration per line; without it `somidl` parses and exits with 1, saying the emitters are not implemented yet.
- **Tests:** unit tests for each construct, errors with their file and line, the command line and Open Watcom's `hello.idl` / `animals.idl` (read in place, with stand-in root IDL); `somidl_parses_open_watcom_s_som_samples` runs the binary; the `somidl` fuzz target (ADR-0112) ran 680 000 inputs without a fault.

## Addendum (2026-11-23): batches 3 and 4, the SDK's SOM headers and the kernel's initializer tables

- **Data exports.** `lx-link` links every import as a call thunk, so a GCC / clang program cannot import SOM's data exports (the root classes' `ClassData` and `CClassData`, `SOMClassMgrObject`, the runtime variables). The kernel gains `WrpSomDataExport(ordinal)` (SOM 3090), giving the address of export 1–19, and `somapi.h` defines each name as a macro for the variable through it (`#define SOMClassMgrObject (*(SOMClassMgr **)WrpSomDataExport(7))`), so it reads and assigns as IBM's does. Open Watcom programs keep importing the data directly.
- **Headers.** `som.h` (the root classes: `ClassData` in the kernel's release orders, method types, `_method` macros), `somapi.h` (types, functions, runtime variables, the protocol's structures) and `somcdev.h` (`SOM_Resolve`, `SOM_ParentNumResolve`, `SOMMethodDebug`, the protocol macros); the root IDL `somobj.idl`, `somcls.idl`, `somcm.idl`, with `somtypes.idl` naming the C types the IDL uses. The release orders are generated from the kernel's tables and a test holds them equal. `check-headers` reads `som*.h` and knows `SOMLINK` (`__cdecl__`, as `APIENTRY`). `som.xh` waits for batch 6.
- **The protocol, as built.** `WrpSomInitCtrlNew(cls)` / `WrpSomInitCtrlFree` (SOM 3091 / 3092). A class's table (made once, freed with an unloaded class file) lists its ancestors once each, post-order (each parent's ancestors before it, parents left to right), the class last; each entry holds the class, the `somDefaultInit` and `somDestruct` it introduced or overrode (0 when inherited) and its data offset. A control block flags every entry but the class's own.
  - A class's own initializer runs the flagged entries before its own, in order; its destructor, after its own work, those before it last first. Called through an ancestor's control block, they find everything before them already done.
  - **An inherited initializer.** When a class has none of its own, `somNew` reaches an ancestor's (or, by the multiple-inheritance merge rule, a later parent's). Run with the ancestor's table, it would miss the classes after the ancestor. So an initializer or destructor called without a control block for an object of another class hands the walk to the kernel, `WrpSomInitWalk(obj)` / `WrpSomDestructWalk(obj, doFree)` (SOM 3093 / 3094), which runs the object's class table itself: every own initializer in order, or destructor last first, each with a control block whose flags are all cleared, then frees the block (and the object for doFree).
  - **Why not in `SOMObject`'s own methods.** The first version walked in `SOMObject`'s `somDefaultInit` / `somDestruct` when they got no control block. `som_ordinal_test` showed the flaw: a class written in SOM 1.x style overrides `somDestruct` and calls its parent's with the null block it was given, so the root walked the table and ran that class's destructor a second time. `SOMObject`'s methods cannot tell such a parent call from a hand-over, so they stay as they were, and the hand-over has its own entries. `somNew` and `somFree` need no walk: by the merge rule, any override in the hierarchy is the one they resolve.
- **Tests:** the kernel's tables over a diamond, the kernel's walk over a merge, the release orders; `c_som_test` with GCC and clang (`som_from_gcc_and_clang_runs_the_initializer_protocol`); the `somidl` tests now read the SDK's root IDL instead of stand-ins.

## Addendum (2026-11-23): batch 5a, the C emitters for single inheritance

Batch 5 is split: 5a covers what Open Watcom's `hello` and a class hierarchy with one parent need; 5b adds attributes, explicit metaclasses, sequences and module-scoped names, which Open Watcom's `animals` needs.

- **Emitters.** `h`, `ih`, `c` and `def` (`-s`, default `h,ih`; `-o dir`, default the current directory). Only the main file's interfaces and types are emitted; an included IDL only declares (the parser now records which file each interface and top-level type came from). The `c` template is never written over an existing file. `def` lists `<Class>NewClass`, `<Class>ClassData` and `<Class>CClassData`, one per line.
- **Usage bindings** (`h`): `<Class>_MajorVersion` / `_MinorVersion`, `typedef SOMObject <Class>`, `somTP_` / `somTD_` method types, `<Class>NewClass`, `<Class>ClassData` (tokens in release order, then methods it does not list) and `<Class>CClassData` (`parentMtab`, `instanceDataToken`), `_<Class>`, `<Class>New()`, `<Class>_<method>` macros and their `_<method>` short forms (unless `SOMGD_<method>` is defined). Interfaces are pointers (`Animal *`), `out` / `inout` arguments pointers, `in` ones values; `callstyle = oidl` drops the `Environment *`. Enums are `unsigned long` with `#define`d values from 1 (CORBA's C mapping); interface-level types and constants are prefixed with the class name.
- **Implementation bindings** (`ih`): `<Class>Data` (the instance variables), `<Class>GetData` (through `somDataResolve` and the instance-data token; a class without data resolves nothing), `_<var>` macros on `somThis`, `<Class>MethodDebug`, the implementations' prototypes (`<functionprefix><method>`; an override takes the declaring ancestor's signature and callstyle), parent calls `<Class>_parent_<Parent>_<method>` and `<Class>_parent_<method>` through `somParentNumResolve`, the `ClassData` definitions, and the class's `somStaticClassInfo` (layout version 1: static methods and overrides; parents and metaclass by name). `<Class>NewClass` calls its parent's and metaclass's `NewClass` (the root classes' with 0.0), then `somBuildClass`.
- **Root IDL.** Root methods take no `Environment *`, so the SDK's `somobj.idl`, `somcls.idl` and `somcm.idl` now say `callstyle = oidl` (they said `idl`).
- **Headers.** The SDK's `somapi.h` and the Open Watcom samples' `som.h` declare `somStaticClassInfo`, `somStaticMethod_t`, `somOverrideMethod_t`, `somBuildClass`, the root classes' `NewClass` and `SOM_ParentNumResolve`; `som.lnk` imports what the bindings call.
- **Refused for now:** attributes and sequences (5b), `any` (S4), multiple inheritance and `init` methods (S3), and the C++ emitters (S2); an override of a method no ancestor declares, a release order naming an undeclared method and an undeclared parent are errors.
- **Tests:** unit tests of each emitter's output and of what is refused; Open Watcom's unchanged `hello.c` and `main.c` build from the generated bindings with Open Watcom (`samples/som_test`, which drops its hand-written `hello.h` / `hello.ih`) and with GCC; `samples/som_idl_test` (GCC and clang) runs 15 checks.

## Addendum (2026-11-24): batch 5b, attributes, metaclasses, sequences and modules; a header for the samples

- **Modules.** An interface in a module has the C name `Module_Interface` (types, macros, `ClassData`, `NewClass`, parent calls `Animals_Dog_parent_Animals_Animal_display`) and the SOM class name "Module::Interface", which `somBuildClass` registers and looks parents and metaclasses up by. Interface-level types are named `<Class>_<name>`. Every class's object type is declared first, so classes may name one another (a forward-declared metaclass).
- **Attributes.** `attribute T x` gives the methods `_get_x` and (unless `readonly`) `_set_x(in T x)`, in the release order like operations, and an instance variable `x` unless `nodata`. The bindings implement the accessors over that variable unless `noget` / `noset`; with `nodata` the program implements them. An override of an ancestor's accessor takes its signature.
- **Metaclasses.** `metaclass = M`: `<Class>NewClass` makes `M` (its `NewClass`) after the parent, and the `somStaticClassInfo` names it; the kernel derives a metaclass when it clashes with the parent's (ADR-0060).
- **Sequences.** `sequence<T>` is the struct `_IDL_SEQUENCE_<T>` (`_maximum`, `_length`, `T *_buffer`; an interface's element is a pointer), declared once before the declarations; `sequence(T)` in `som.h` names it.
- **The samples' header.** 5a put the declarations generated bindings need into the Open Watcom samples' `som.h`; three samples declare some of them themselves and stopped compiling with every compiler (CI caught it; the local gate had run prebuilt samples). They now live in `samples/som_test/include/somapi.h`, which the generated `.h` includes after `som.h`, as the SDK's `som.h` already includes its `somapi.h`. The generated files also depend on the `somidl` binary, so a newer one regenerates them.
- **Result.** Open Watcom's `animals` (metaclasses with data and constructors, a derived metaclass, attributes, sequences, module-scoped names) builds unchanged from generated bindings with Open Watcom, GCC and clang, and `samples/som_test` drops its hand-written `animals.h` / `animals.ih`. Its `classes` sample keeps hand-written bindings: it is written in OIDL (`.csc`), which `somidl` does not read.

## Addendum (2026-11-24): batch 6, the C++ emitters (stage S2)

- **`xh`, `xih`, `xc`.** The usage bindings declare each class as a C++ class deriving from its parent's (the root classes' come from `som.xh`), with `SOM_CPP_CLASS` (ADR-0061: `new` through `somNewNoInit`, the constructor of the object's own class calls `somDefaultInit`, `delete` through `somDestruct(1)`) and each method an inline member calling through the `ClassData` token with `this`. The method types, `ClassData`, `CClassData` and `_<Class>` are as in C; the C `_method` macros and `<Class>New()` are not emitted. `xih` is the C implementation binding over the `.xh`; `xc` the template over the `.xih`, never overwriting a file.
- **C++ text.** The generated static data names are `static char x_t[] = "…"; static char *x_s = x_t;` in both languages, since C++ does not bind a string literal to `char *`.
- **SDK.** `sdk/include/som.xh`, from the Open Watcom samples' (ADR-0061), with the protocol's typed control blocks (`somInitCtrl *`, `somDestructCtrl *`) and `somCheckVersion`.
- **Tests.** Open Watcom's `hellocpp` builds unchanged from generated bindings with Open Watcom (`samples/som_test`, whose `cpp` check now derives from that generated `Hello`; the hand-written `hello.xh` / `hello.xih` are gone) and with GCC and clang (`samples/som_idl_test`); a unit test checks the emitted C++.

## Addendum (2026-11-24): batch 7, stage S3

- **Multiple inheritance.** A class may name several parents: each one's `NewClass` runs first, the class information lists them all (by SOM name), and an override gets a parent call for each parent that has the method (`<Class>_parent_<Parent>_<method>`, through `somParentNumResolve` with that parent's number from 1) and `<Class>_parent_<method>` for the first. In C++ a SOM object has one method-table word, so the class derives from its first parent only and forwards the later parents' methods (those not reachable through the first) as inline members.
- **`directinitclasses`.** `directinitclasses = "A, B"` (or a single name) is checked against the declared interfaces; `<Class>NewClass` passes the classes to the kernel after `somBuildClass` through Warpine's `WrpSomSetDirectInitClasses(cls, n, somId *)` (SOM 3095), which makes them, in that order, the ancestors the class's table visits instead of its parents (each must be an ancestor; the table is made again).
- **`init` methods.** An operation with the `init` modifier must return `void` and take `inout somInitCtrl ctrl` first (the SOM Programming Guide's form); its implementation uses `SOM_InitBegin` / `SOM_InitEnd` like `somDefaultInit`, so the ancestors' default initializers run first. C code makes an object with `_somNewNoInit` and calls it with a null control block; in C++ it is also a constructor taking its other arguments (and the `Environment` for `callstyle = idl`).
- **Copy and assign.** `SOMObject`'s `somDefaultCopyInit`, `somDefaultConstCopyInit`, `somDefaultVCopyInit`, `somDefaultConstVCopyInit`, `somDefaultAssign`, `somDefaultConstAssign`, `somDefaultVAssign` and `somDefaultConstVAssign` (SOM 3214–3221, in the release order's places) no longer raise `SOMERROR_NotImplementedYet`. With a control block they do nothing (`SOMObject` has no data); without one, they copy the whole instance shallowly from an object of the same class or a descendant; the assigns return the object. A class overrides them in IDL like any `SOMObject` method; `som.h` has `_somDefaultCopyInit` / `_somDefaultAssign`.
- **Samples.** `som_idl_test`'s default build directory is now `build/`: generated headers beside `counter.c` were found by its quoted `#include` before fresh ones.
- **Tests.** Kernel unit tests for copy / assign and direct-init classes; an emitter test for two parents, `directinitclasses`, an `init` method and the C++ forwarding and constructor; `som_idl_test` (25 checks: two parents, the visit order, the `init` method, copy / assign) with GCC and clang.
