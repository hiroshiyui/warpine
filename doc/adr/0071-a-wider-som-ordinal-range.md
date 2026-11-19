# ADR-0071: The SOM module is 4096 ordinals wide; Warpine's own exports move to 3072–4095

- **Status:** Accepted; amends ADR-0070 (SOM's ordinals), on the private range
- **Date:** 2026-11-20
- **Sources:**
  - The code:
    - `src/loader/constants.rs` (`SOM_ORDINALS`, `STUB_AREA_SIZE`, the bounds checks);
    - `src/loader/som.rs` (the private ordinals, `SOM_LAST_PRIVATE`);
    - `src/loader/api_registry.rs`, `targets/os2api.def`, `src/loader/api_trace.rs`;
    - `src/bin/gen_api.rs` (its copy of `STUB_AREA_SIZE`).
  - ADR-0070.

## Context

ADR-0070 left IBM's later SOM exports the ordinals 48–511 and put Warpine's own at 512–1023, inside the 1024 ordinals every built-in module has. Nothing public gives IBM's highest SOM ordinal. If IBM's numbers reached 512, they would collide with Warpine's.

The 1024-ordinal width is Warpine's own layout of the stub area, not an OS/2 limit. The stub area lies between `MAGIC_API_BASE` and `DYNAMIC_ALLOC_BASE` (16 MB). Its only hard bound is the 64 KB that 16-bit NE programs reach with 16-bit offsets (`STUB_AREA_SIZE <= u16::MAX`). NE programs never import SOM.

SOM is the last module in the stub area, so it can grow without moving another module.

Warpine's private SOM exports are almost all internal: the method thunks, the procedures the runtime variables hold, and the method-error procedure. Programs never import them; they are addresses written into method tables and variables. Only `WrpSomMetaclassFor` is imported by programs, and by name.

## Decision

- **A wider module.** The SOM module is `SOM_ORDINALS` = 4096 ordinals wide, so `STUB_AREA_SIZE` is `SOM_BASE + SOM_ORDINALS` (26 624). The other modules keep 1024 each.
- **IBM's range.** Ordinals 48–3071 are left to IBM's later exports.
- **Warpine's own exports at 3072–4095:**
  - the SOM 2.x functions whose IBM numbers are unknown at 3072–3076;
  - the procedures the runtime variables hold at 3080–3089;
  - `WrpSomMethodError` at 3199;
  - the methods of `SOMObject`, `SOMClass` and `SOMClassMgr` from 3200, 3300 and 3400.
- **Bounds.** Compile-time assertions keep the private range at 3072 or above and below `SOM_ORDINALS`, and the module inside the stub area. The stub area stays under the 64 KB NE bound. The unit test that checks IBM's range checks against the new bounds.

## Consequences

- **Room for IBM.** IBM's SOM exports up to 3071 can be added without renumbering Warpine's.
- **The cost.** The stub area grows by 3 KB of `INT 3` bytes.
- **Nothing changes for programs.** Programs importing by IBM's ordinal (1–47) or by name do not change. Nothing imports the moved entries by number.
- **Tests:** the SOM unit tests (with the new numbers), `samples/som_ordinal_test` and `samples/som_test`, unchanged.
