# ADR-0063: Module lifecycle — failed startup routines stop the program, OS/2 paths for DosLoadModule, zeroed memory, SOM class unloading

- **Status:** Accepted; supersedes ADR-0059's "a `_DLL_InitTerm` failing at process start does not stop the program"
- **Date:** 2026-11-19
- **Sources:**
  - Loading:
    - `src/loader/lx_loader.rs` (`find_module_file`, zeroed objects);
    - `src/loader/warpine_api.rs` (`start_inits_done`);
    - `src/loader/pm_hook.rs` (`ChainEnd::Exit`, `ChainThen::StartProcess`);
    - `src/loader/vcpu.rs`.
  - Memory: `src/loader/managers.rs` (`MemoryManager::alloc_reporting`, `high_water`), `src/loader/guest_mem.rs` (`Loader::alloc_zeroed`).
  - The VFS: `src/loader/vfs_hostdir.rs` (`to_host_path` through `enforce_sandbox`).
  - SOM: `src/loader/som.rs` (`som_unregister`, `som_unload_class_file`, `SomRuntime::modules` / `ready_order`).
  - Samples: `samples/dllinit_test` (new), `samples/som_test/findcls` (extended).
  - IBM *Control Program Programming Reference* (`DosLoadModule`, `DosAllocMem`: committed memory is initialized to zero, `ERROR_INIT_ROUTINE_FAILED`); *SOM Programming Reference* (`somUnregisterClass`, `somUnloadClassFile`).
  - Earlier decisions: ADR-0059, ADR-0062, ADR-0026 (call chains).

## Context

Classes that SOM can load (ADR-0062) should also be unloadable. Trying that showed what the loader still lacked:

- **Stale memory.** A DLL loaded again into memory another module had used kept that module's data: its static variables, which should start at zero, did not. The memory manager hands out freed blocks as they are. Its bump allocator does too, since freeing the top block lowers its next address. So `DosAllocMem` and a module's zero-filled pages could show what an earlier allocation left, although OS/2 commits memory zeroed.
- **Failed startup routines.** A program whose DLL's `_DLL_InitTerm` failed at start ran anyway (ADR-0059). OS/2 does not start such a program.
- **OS/2 paths.** `DosLoadModule` accepts a module's full path (`C:\DIR\X.DLL`). Warpine never resolved such names, and since ADR-0062 refuses them in the host search.
- **An unconfined path call.** `HostDirBackend::to_host_path`, behind `DriveManager::resolve_to_host_path`, skipped the sandbox check the backend's other calls make. On a drive whose host directory folds case in the kernel, `..` could leave the drive; in either mode, a symbolic link could. `DosExecPgm`, `WinStartApp`, CMD.EXE and the MSG calls use it.

## Decision

### Memory

- **Every block reports what may be stale.** `MemoryManager::alloc_reporting` returns the block and how many of its bytes may hold earlier contents:
  - a reused freed block: all of it;
  - a bump block: the part below the highest address ever handed out (`high_water`).
- **`Loader::alloc_zeroed`** clears those bytes. Memory never handed out is still zero from the start, and is not touched.
- **Who uses it:** `DosAllocMem`, `DosAllocSharedMem` outside the shared arena, and a DLL's objects.
- **Who does not:** thread stacks and the runtime's internal blocks, which their users write in full or clear themselves.

### Starting a program

- **A failed startup routine stops the program.** When a `_DLL_InitTerm` called by `WrpStartProcess` returns 0, the chain ends in `ChainEnd::Exit(ERROR_INIT_ROUTINE_FAILED)`. That enters `DosExit(EXIT_PROCESS, 295)` as if the program had called it, so its exit list runs. A line on standard error says why; the host exit code is the low byte, 39.
- **The message is Warpine's own wording;** no OS/2 message number is claimed.

### Module names

- **`Loader::find_module_file`** resolves a module name for `DosLoadModule` and SOM's class files:
  - a plain name is searched beside the program and in `C:\OS2\DLL` (ADR-0062);
  - anything else is an OS/2 path, resolved through the drives.

  The module from an OS/2 path is named by its file's base name, so `C:\LIBS\GOODINIT.DLL` is module `GOODINIT`; an already loaded module of that name is reused.
- **`DosFreeModule` of a handle of no loaded module** (never one, or already freed) returns `ERROR_INVALID_HANDLE` rather than `NO_ERROR`.
- **`to_host_path` runs `enforce_sandbox`.** A path that leaves its drive's directory, through `..` or a symbolic link, resolves to nothing, for every caller.

### Unloading SOM classes

- **The runtime remembers each module's classes.** It records the order classes become ready (`ready_order`). When the class manager has loaded a module, the classes made since the loading began are that module's (`modules`).
  - With the default `SOMLoadModule`, the handle is known.
  - With a program's own `SOMLoadModule`, the handle is the one it wrote.
- **`somUnregisterClass(cls)`** removes the class from the registry, so `somFindClass` looks for it again. It returns 0, or 1 when the class was not registered.
- **`somUnloadClassFile(cls)`:**
  - unregisters every class the module of `cls` made;
  - withdraws their method tables, so their surviving objects resolve every method to the error procedure rather than into freed code;
  - frees the module through `SOMDeleteModule` (the default is `DosFreeModule`; a program's own through a call chain).

  It returns that result, or 1 for a class no loaded module made.

### Warpine's choices (unverified)

- **Exit code and message.** The exit code (295) and the message for a program whose DLL failed to start.
- **Return values.** 1 from `somUnregisterClass` / `somUnloadClassFile` for an unknown class.
- **What happens to objects.** Unloading withdraws the classes' method tables. Whether IBM's SOM frees the class objects is not known; Warpine keeps them.
- **Attribution.** Classes made by another thread while a module loads count as that module's.

## Consequences

- **Zeroed memory.** Programs get zeroed memory from `DosAllocMem` however often memory is reused, and a reloaded DLL starts clean.
- **Failed startup.** A program whose DLL fails to initialize ends before its first instruction.
- **OS/2 paths.** `DosLoadModule` works with OS/2 paths on Warpine's drives, without reaching outside them.
- **Unloading.** A SOM class's module can be unloaded and the class loaded again.
- **Tests:**
  - `dllinit_test` covers startup routines, OS/2 paths, exports by name and names outside drive C;
  - `findcls` covers unregistering, unloading and reloading;
  - unit tests cover zeroing, the sandboxed host paths, `find_module_file` and the exit chain.
