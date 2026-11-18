# ADR-0044: Run a container sort as a resumable merge sort, one comparison per call chain

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** `src/loader/pm_container.rs` (`SortJob`, `cnr_start_sort`, `container_sort_step`, `sort_chain`), `src/loader/pm_hook.rs` (`ChainThen::ContainerSort`, `ChainEnd::Next`), `src/loader/vcpu.rs` (`continue_call_chain`); `samples/pm_cnrtree_test`; `<pmstddlg.h>` (`CM_SORTRECORD`, `CNRINFO.pSortRecord`); IBM *PM Programming Reference* (`CM_SORTRECORD`: `SHORT pfnCompare(PRECORDCORE, PRECORDCORE, PVOID pStorage)`); ADR-0026

## Context

`CM_SORTRECORD` sorts a container's records with a comparison function in the application. A `CNRINFO.pSortRecord` function does the same for every insertion. Both need many guest calls, about n log₂ n of them, before the message returns. Each call's answer decides which pair is compared next.

ADR-0026 runs guest code from an API as a call chain: a list of calls fixed when the chain starts, then a return or a re-run of the API. Neither fits as it stands:
- The comparisons cannot be listed in advance.
- Re-running `WinSendMsg` per comparison would redo the whole sort on the host each time, or need a cache of answers. That is quadratic in the number of comparisons, and the send hooks would run again on every re-run.

## Decision

- **A resumable sort.** `SortJob` is a stable bottom-up merge sort of each sibling group (the top level and every parent's children), driven from outside. `step(answer)` takes the answer to the pair it asked last and returns the next pair, or `None` when every group is sorted. Its state is a few indices into two buffers, so each step costs O(1) host work.
- **One comparison per chain, linked by `ChainEnd::Next`.**
  - `CM_SORTRECORD` (or an insertion with `pSortRecord`) stores the job in the container (`CnrState::sort`, with an id) and returns a call chain of one step: `pfnCompare(p1, p2, pStorage)`. Its `ChainThen::ContainerSort` names the container and the job.
  - When the call returns, `finish_chain` feeds the `SHORT` result to the job. If the job asks for another pair, it returns `ChainEnd::Next` with the next one-step chain, which `continue_call_chain` starts at once. Otherwise it returns the result to the caller.
  - The chain mechanism stays the one of ADR-0026. It gains only a way to continue with another chain.
- **The order is applied at the end, and only if it still holds.** The sorted groups replace the records' places only if every group still holds the same records. A comparison function that inserts or removes records leaves the order alone, and `CM_SORTRECORD` then returns FALSE. An insertion returns its count whatever happens. A job whose container is destroyed, or that a newer sort replaced (the ids differ), ends with FALSE.
- **Comparisons go to the guest one at a time.** The function is application code and may use its `pStorage` and the records freely.
- **pStorage for insertions is NULL** (unverified: what PM passes for `pSortRecord` sorts).

## Consequences

- Sorting costs about n log₂ n guest calls, each one VM entry and exit, and O(n) host memory per container. A 10,000-record sort makes about 130,000 short calls.
- The sort is stable, so equal records keep their item order. PM's own stability is not documented.
- Other APIs that need guest answers to decide their next step, such as `CM_FILTER`, could use the same pattern: a job stored with the control, and one-call chains linked by `ChainEnd::Next`.
- A sort requested from a context that drops a chain result (a message dispatched to a built-in control from inside another chain) does not run. Its job stays until the next sort replaces it.
