# ADR-0043: Keep container records in guest memory, carved from per-container arenas

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** `src/loader/pm_container.rs` (`RecordHeap`, `CnrState`, `container_message`), `src/loader/pm_destroy.rs` (freeing on destruction); `samples/pm_container_test`; `<pmstddlg.h>` (`RECORDCORE`, `MINIRECORDCORE`, `FIELDINFO`, `CNRINFO`, `RECORDINSERT`, `FIELDINFOINSERT`); IBM *Presentation Manager Programming Reference* (`CM_ALLOCRECORD`, `CM_FREERECORD`, `CM_INSERTRECORD`, `CM_REMOVERECORD`, `CM_ALLOCDETAILFIELDINFO`) and *PM Programming Guide* ("Container Controls")

## Context

Every control so far keeps its items on the host: list-box strings, notebook pages and value-set items are copied in by a message and read out by another. The container is different.

- `CM_ALLOCRECORD` returns **pointers into the application's memory**: a chain of `RECORDCORE`s (or `MINIRECORDCORE`s with `CCS_MINIRECORDCORE`), linked by `preccNextRecord`, each followed by `cbRecordData` bytes of the application's own data.
- The application fills these records itself (`pszText`, `pszName`, `ptlIcon`, its own fields after the core) and reads them back, including `flRecordAttr`, which the container updates as records are selected.
- Detail view goes further. A `FIELDINFO`'s `offStruct` names an offset inside each record where the column's data lives: a `PSZ`, a `ULONG`, a `CDATE` or a `CTIME`.
- Programs commonly allocate one record per call. A real file manager inserts thousands.

The guest memory manager (`MemoryManager`) hands out page-sized blocks. A block per `CM_ALLOCRECORD` would spend 4 KB on a 56-byte record, and the dynamic pool would run out after a few thousand records.

## Decision

- **Records live in guest memory, and the guest's copy is the only copy.** The container keeps no host copy of a record's texts, position or attributes. It reads them through bounded `guest_read` calls when it paints, searches or answers a query. It writes only `flRecordAttr` (emphasis), as PM does, and the core fields it initialises on allocation. A change the application makes to a record shows the next time the container paints, which is what `CM_INVALIDATERECORD` asks for.
- **Allocation from per-container arenas (`RecordHeap`).**
  - Each container takes arenas of `CNR_ARENA_BYTES` (64 KB) from the memory manager, or one exact arena for a larger request.
  - A `CM_ALLOCRECORD` / `CM_ALLOCDETAILFIELDINFO` call carves its records from the current arena, zeroed and chained.
  - Every record and field info is registered by address, with its size and arena.
  - Freeing an item decrements its arena's live count. An arena whose count reaches zero goes back to the memory manager.
  - Space freed inside a live arena is not reused (no free list).
- **Only registered pointers are accepted.** Insertion, removal, freeing, emphasis, queries and `offStruct` reads all check that a record or field info was allocated by this container. The checks also cover that it is (or is not) inserted, and that an offset plus its field size stays within the record. A stray pointer from the guest therefore never makes Warpine read past a record or free memory that is not the container's. Item counts are bounded (`MAX_CNR_ITEMS`, `MAX_RECORD_DATA`).
- **Order and hierarchy are host-side.** The insertion order and each record's parent are kept in `CnrState`, not in `preccNextRecord`. That field is read only to walk the chain `CM_INSERTRECORD` is given. `CM_QUERYRECORD` answers from the host-side order.
- **Destruction frees everything.** Destroying a container queues all its arenas on `WindowManager::pending_guest_frees`. The window manager cannot take the memory manager's lock, so `run_destroy` returns them to the memory manager after the destroy chain ends.
- **Lock order:** the memory manager is never locked while the window manager is held. An allocation first takes an arena from the memory manager, then registers it under the window-manager lock.

## Consequences

- Programs that keep pointers to their records, write their fields directly or embed their own data after the core work unchanged. That is how every container program is written.
- Many small allocations share pages. 10,000 single-record allocations of `RECORDCORE` + 32 bytes take about 1.4 MB instead of 40 MB.
- An application that keeps allocating and freeing records leaves holes in partly used arenas until all of their records are freed. The cost is bounded by the live records times the arena size.
- Records are owned by one container. PM (Warp 3 and later) lets a record be inserted into several containers ("shared records"), which Warpine does not support yet; inserting another container's record fails.
- Unverified (no public source): whether `RECORDCORE.cb` counts the application's extra bytes (Warpine sets it to the core size); whether the container frees inserted records when it is destroyed (Warpine frees them).
