# ADR-0005: Dispatch API calls through INT 3 thunks with one ordinal slot per built-in module

- **Status:** Accepted
- **Date:** 2026-03-14
- **Sources:** `CLAUDE.md` (Execution flow, Important constants); `src/loader/descriptors.rs` (`resolve_import`); `src/loader/constants.rs`; developer guide (module table)

## Context

OS/2 programs import their API by module name and ordinal. Warpine implements the system DLLs on the host, so every imported entry point needs an address in guest memory that hands control to Warpine.

## Decision

Imports of built-in modules resolve to stub addresses in an area at `MAGIC_API_BASE` (0x01000000) filled with INT 3. A call traps to Warpine (VM exit), which derives the module and ordinal from the address and dispatches it: a static sorted registry (`api_registry.rs`) for most modules, sub-dispatchers for VIOCALLS, KBDCALLS, MOUCALLS, PMWIN and PMGPI.

- Each built-in module has a 1024-ordinal slot (`*_BASE` constants: DOSCALLS 0, QUECALLS 1024, PMWIN 2048 … MOUCALLS 15360, PMSHAPI 16384, HELPMGR 17408) inside `STUB_AREA_SIZE`.
- A new module takes the next slot and grows `STUB_AREA_SIZE`; compile-time asserts keep the slots 1024 wide, below `DYNAMIC_ALLOC_BASE` and reachable by the NE loader's 16-bit offsets.
- `DosLoadModule` / `DosQueryProcAddr` treat the built-in modules as loaded DLLs (`LoadedDll::is_builtin`).

## Consequences

- Adding an API is a registry entry or a dispatcher arm plus its implementation.
- Every list of modules must follow a new slot: `resolve_import`, the dispatchers, `api_trace.rs`, `BUILTINS`, `lx_link`, `gen_api` and `targets/os2api.def` (see the developer guide).
- A DLL with a built-in module's name is always served by Warpine (`DosLoadModule` checks `BUILTINS` before searching the host); other DLLs supplied with a program load normally.
