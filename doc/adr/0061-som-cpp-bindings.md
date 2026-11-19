# ADR-0061: SOM's C++ bindings create objects through the runtime; somDefaultInit and somDestruct

- **Status:** Accepted (SOM stage 3: the C++ bindings)
- **Date:** 2026-11-19
- **Sources:**
  - The runtime: `src/loader/som.rs` (`somDefaultInit` 112, `somDestruct` 113, `somNew`)
  - Sample `samples/som_test`:
    - hand-written C++ bindings `include/som.xh`, `hellocpp/hello.xh` and `hellocpp/hello.xih`, over the unchanged C++ sources of Open Watcom's `hellocpp` SOM sample (`vendor/watcom/samples/os2/som/hellocpp`);
    - Warpine's own check `cpp/` (`Loud`, a class derived from `Hello`)
  - IBM *SOM Programming Guide* ("Using SOM classes in C++", "Initializing and uninitializing objects") and *SOM Programming Reference* (`somNewNoInit`, `somDefaultInit`, `somDestruct`), as on EDM/2
  - Earlier decisions: ADR-0059 (the SOM kernel), ADR-0060 (tokens, metaclasses), ADR-0026 (call chains)

## Context

Open Watcom's `hellocpp` sample is the C++ version of `helloc`:
- `new Hello` creates a SOM object;
- `obj->sayHello(ev)` calls a SOM method as a C++ member function;
- `obj->somFree()` frees the object.

It is the first C++ program to run on Warpine. Its bindings (`hello.xh`, `hello.xih`) and the toolkit's `som.xh` come from IBM's SOM compiler, which the clean-room policy keeps out (ADR-0059), so they are written by hand.

In SOM 2.1, creating an object is split in two:
- `somNewNoInit` allocates the object;
- `somDefaultInit` initializes it.

The C++ bindings use this split: `operator new` allocates and the constructor initializes. A C++ object of a derived class runs the constructor of every base class, but the SOM object must be initialized only once.

## Decision

### The runtime

- **`somDefaultInit(self, ctrl)`** (`SOMObject`, ordinal 112) calls the object's `somInit`, so classes written for SOM 1.x, which initialize in `somInit`, keep working. The control block is not used.
- **`somDestruct(self, doFree, ctrl)`** (113) calls the object's `somUninit`, then frees the object when `doFree` (an octet) is set. An object the runtime did not make is left alone.
- **`somNew`** is `somNewNoInit` followed by the object's `somDefaultInit`. When `somDefaultInit` is not overridden, it calls `somInit` directly, without the extra call.
- **The `SOMObjectClassData` fields** `somDefaultInit` and `somDestruct` follow `somDumpSelfInt`.

### The C++ bindings (`som.xh`)

- **A SOM class is a C++ class:**
  - its only member is the method-table word;
  - it has no virtual functions;
  - its methods are inline functions that call through `ClassData` tokens, like the C bindings.

  `SOMObject`, `SOMClass` and `SOMClassMgr` are defined this way. `som.h` serves both languages: under C++ it declares the classes instead of `typedef SOMAny`, and its functions and data get C linkage (`SOMEXTERN` is `extern "C"`).
- **Every class gets `SOM_CPP_CLASS(cls)`:**
  - `operator new` calls `somNewNoInit` on the class object, creating the class first;
  - the constructor calls `somDefaultInit` only when the object is a SOM object (`somIsObj`) of exactly that class. The constructors of its C++ base classes see another class and do nothing, so initialization runs once;
  - `operator delete` calls `somDestruct(1)`.
- **Dispatch is by SOM.** An override is reached through a pointer to a base class, because methods resolve through the object's SOM class, not its C++ type.
- **Stack and static objects** are not SOM objects: their constructors see none and leave them alone.

### Warpine's choices (unverified against IBM's SOM)

- `som.xh` and the `.xh` / `.xih` layouts as a whole, `SOM_CPP_CLASS`, and the constructor's exact-class test. IBM's bindings may decide differently, for instance by comparing method tables.
- That root `somDefaultInit` calls `somInit`, and that `somDestruct` calls `somUninit`.
- The order of the new `ClassData` fields.
- Objects on the stack: SOM 2.1's C++ bindings can make them, and Warpine's cannot.

## Consequences

- **`hellocpp` runs.** Open Watcom's `hellocpp` builds unchanged with `wpp386` and runs; `helloc`, `classes` and `animals` still run.
- **The `cpp/` check (`sompp.exe`)** verifies:
  - a derived class's `somInit` runs once;
  - overrides are reached through a base pointer;
  - parent calls and instance data work;
  - `delete` runs `somUninit` and frees the object;
  - a stack object is no SOM object.
- **Open Watcom's C++ runtime** starts and runs on Warpine; these are its first C++ programs there.
- **Still missing:**
  - stack and static SOM objects;
  - the initializer control blocks (`somInitCtrl`, `somDestructCtrl`) and the `somInit` / `somUninit` ordering they control across several parents;
  - `somFindClass` loading a class's DLL;
  - `SOMError` / `SOMOutCharRoutine`;
  - IBM's ordinals and layouts.
