# ADR-0060: SOM tokens name their introducing class; derived metaclasses, multiple inheritance and a runtime heap

- **Status:** Accepted (SOM stage 2); supersedes ADR-0059's token encoding and its single inheritance
- **Date:** 2026-11-19
- **Sources:**
  - The runtime: `src/loader/som.rs` (`SomRuntime::inherit`, `som_metaclass_for`, `SomHeap`, `c_format`, `som_data_resolve`)
  - Sample `samples/som_test/animals`: hand-written bindings (`animals.h`, `animals.ih`) over the unchanged C sources of Open Watcom's `animals` SOM sample (`vendor/watcom/samples/os2/som/animals`); `include/som.h`
  - IBM *SOM Programming Guide* ("Metaclasses", "Derived metaclasses", "Multiple inheritance", "Customizing memory management") and *SOM Programming Reference* (`somInitMIClass`, `somPrintf`, `SOMMalloc` / `SOMCalloc` / `SOMRealloc` / `SOMFree`, `SOM_TraceLevel`, `somPrintSelf`, `somDumpSelf`, `somDumpSelfInt`), as on EDM/2
  - Earlier decisions: ADR-0059 (the SOM kernel), ADR-0026 (call chains)

## Context

Open Watcom's `animals` sample is the next program to run. It uses what stage 1 left out:

- **Metaclasses with data and methods.** `M_Animal` keeps the sequence of live animals in each class object and has the constructor `newAnimal`; its `somInit` runs when a class object is made.
- **A derived metaclass.** `Dog` asks for the metaclass `Kennel`, while its parent `Animal` has `M_Animal`. Neither metaclass descends from the other, so SOM derives one from both: `Kennel`'s `newDog` calls `M_Animal`'s `newAnimal` on the same class object. The derived metaclass has two parents.
- **The runtime's own services:**
  - the allocation variables `SOMMalloc`, `SOMRealloc` and `SOMFree`;
  - `somPrintf`;
  - `SOM_TraceLevel`;
  - the `somDumpSelfInt` override.

Stage 1's tokens could not serve two parents:
- **Method tokens** were indices into a class's method list, which only work while each class extends a single parent's list.
- **Data tokens** were offsets into the object, which differ between classes once a class has two parents.

## Decision

### Tokens

- **Method tokens.** A method token names one method introduced by one class: its index in the runtime's list of all methods, plus one. Each class maps the tokens it supports to its procedures (`SomClass::slots`). So a token works on the introducing class and on every descendant, through any parent.
- **Data tokens.** A data token is the introducing class's object.
  - Each class lists where every ancestor's instance data lies in its instances (`SomClass::parts`).
  - `somDataResolve(obj, token)` looks the token up in the object's class. An unknown token gives 0.
  - `somGetInstanceToken` returns the class.

### Multiple inheritance

- **The call.** `somInitMIClass(self, inherit_vars, name, parents, dataSize, …)` takes the parents as a sequence; `somInitClass` is the one-parent case.
- **Methods** (`SomRuntime::inherit`):
  - a class starts with its first parent's methods;
  - later parents add the methods it lacks;
  - a later parent's override replaces an implementation that the earlier parents left at the introducer's.
- **Data.** Each ancestor's instance data is laid out once, in the order the parents are met, 4-byte aligned. `inherit_vars` and `dataAlignment` are not used.
- **Parents.** `somGetParent` gives the first parent.
- **Bounds.** At most `MAX_PARENTS` (16) parents.

### Metaclasses

- **Class objects carry the metaclass's data.** A class object is an instance of its metaclass: `somNew` on the metaclass allocates the metaclass's instance size and runs its `somInit`, which may be the program's override.
- **Choosing the metaclass.** A binding asks for the metaclass of a new class with `WrpSomMetaclassFor(requested, parent)`, a Warpine function in `SOM.DLL`:
  - with no request, the parent's metaclass;
  - the requested metaclass, if it descends from the parent's;
  - the parent's metaclass, if it descends from the requested one;
  - otherwise a derived metaclass with both as parents. It is made once per pair, named `Requested+ParentMeta` and has no data of its own.

  The binding macro `SOM_NEW_META_CLASS` (`som.h`) calls it before `somNew`, because the class object must be allocated at the derived metaclass's size.

### The runtime's services

- **The heap.** `SomHeap` carves 64 KB arenas from the memory manager into blocks rounded to 16 bytes and keeps freed blocks by size; a block over 32 KB gets pages of its own. Objects, method tables, `somId`s, names and `SOMMalloc` blocks all come from it, so an object no longer takes a page.
- **Allocation variables.** `SOMMalloc`, `SOMCalloc`, `SOMRealloc` and `SOMFree` are exported data holding the addresses of the runtime's procedures (ordinals 12–15), so a program calls through them and may replace them.
- **Levels.** `SOM_TraceLevel`, `SOM_WarnLevel` and `SOM_AssertLevel` are exported `long`s. The bindings' `SOM_MethodDebug` prints a line per method entered while `SOM_TraceLevel` is set.
- **`somPrintf`** formats like C's `printf` (`c_format`: integer, string, character, pointer and floating conversions, flags, width, precision, lengths). It reads at most nine argument words from the call and writes at most 64 KB to standard output.
- **New `SOMObject` methods.**
  - `somPrintSelf` writes `{An instance of class C at address A}`.
  - `somDumpSelf(level)` writes that line indented, then calls the object's `somDumpSelfInt`.
  - `somDumpSelfInt` at the root writes nothing.

### Warpine's choices (unverified against IBM's SOM)

- **Encodings and order.**
  - the token encodings;
  - the order of the new root-class `ClassData` fields: `somPrintSelf`, `somDumpSelf` and `somDumpSelfInt` after `somRespondsTo`; `somInitMIClass` after `somSupportsMethod`.
- **Multiple inheritance.** The resolution rule for methods overridden by several parents.
- **Derived metaclasses.** Their name, and `WrpSomMetaclassFor` itself: IBM's bindings call `somBuildClass`, whose static class information is not published.
- **Output.** What `somPrintSelf`, `somDumpSelf` and method tracing print.
- **Allocation.** `SOMMalloc` returns zeroed memory.

## Consequences

- **`animals` runs.** Open Watcom's `animals` sample runs unchanged and prints all nine animals:
  - constructors on metaclasses (`newAnimal`, `newDog`);
  - instance tracking in a class object's data;
  - a derived metaclass;
  - overridden attribute methods;
  - three levels of overrides of `talk` / `display`.

  Every animal is freed through its `somFree` override. `helloc` and `classes` still run.
- **Any number of parents.** Classes may have several parents.
- **Smaller allocations.** The runtime uses far less guest memory per object.
- **Still missing:**
  - the C++ bindings (`hellocpp`);
  - `somFindClass` loading a class's DLL;
  - `SOMError` / `SOMOutCharRoutine`;
  - IBM's ordinals and layouts;
  - `SOMU`, the interface repository, DSOM;
  - the Workplace Shell classes.
