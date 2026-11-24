# ADR-0125: No address randomisation inside the guest, for now

- **Status:** Accepted; answers the "Address layout" item of the Security Facilities TODOs
- **Date:** 2026-11-24
- **Sources:** ADR-0020 (OS/2's address spaces: DLL code and shared memory at one address in every process), ADR-0049, ADR-0066, ADR-0094, ADR-0111, ADR-0113, ADR-0114, ADR-0124; `src/loader/constants.rs`, `src/loader/managers.rs` (the allocator).

## Context

Inside a guest everything sits at fixed addresses: the program's objects at their link bases (`EXE_AREA_START`…, ADR-0094), Warpine's system area, the paging arena and the API thunks at constants, the allocator handing out the lowest free range from `DYNAMIC_ALLOC_BASE`, and DLLs loaded through the allocator in load order. The TODO asked whether to randomise the allocator's and the DLLs' bases, as host operating systems do with ASLR.

Randomisation makes it harder to exploit a memory bug in a program: an attacker who controls input to a guest program (a networked OS/2 application, a crafted file) must guess where code and data are.

## Decision

Warpine does not randomise addresses inside the guest. Every layout stays deterministic.

Reasons:
1. **The guest is not the boundary Warpine defends.** A guest that is fully compromised can do only what the guest could do anyway: run at ring 3 under KVM (ADR-0113), reach its own drives (ADR-0066), behind Landlock (ADR-0111) and the system-call filter (ADR-0124). Randomisation would protect a guest program from its own bugs, not the host from the guest. Warpine's host process keeps the host's own ASLR (its code, libraries and the guest memory mapping are placed by Linux).
2. **Little entropy.** The guest has 256 MB; the allocator's region is 192 MB. Page-granular placement there gives at most about 15 bits, less for large blocks and DLLs, and a 32-bit guest has no room to add more. Spraying or a brute-force retry defeats that easily.
3. **OS/2 programs expect stable layouts.** On OS/2 a DLL's code and every shared block have one address in every process (ADR-0020); Warpine keeps that for shared memory (ADR-0049) and must keep it if DLLs ever become shared. Executables are linked at a base and often have no internal fixups to move them.
4. **Determinism has value here.** The golden pictures, the integration tests, crash reports and `--gdb` sessions with `.dbg` companions all read better when a run repeats exactly.

## Warpine's choices (unverified)

- Whether real OS/2 randomised anything: it is not known to, and Warpine does not try to match an unknown behaviour.

## Consequences

- No change in code. The Security Facilities TODO item is closed by this record.
- **Revisit when** Warpine runs networked OS/2 programs for others to reach (a server), or a guest program's compromise starts to matter on its own, for example when guests hold secrets of their own. A per-process random start of the allocator's region, opt-in (`WARPINE_ASLR`), would be the first step; DLL bases would follow only while DLLs stay per process.
