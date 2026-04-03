---
name: code-review
description: Perform a structured code review of staged or recently changed Warpine source files, covering correctness, security, test coverage, clean-room compliance, and documentation hygiene.
---

When performing a code review, follow these steps in order:

## 1. Identify the scope

Determine which files to review:
- If the user specifies files or a PR, review those.
- Otherwise use `git diff HEAD` (unstaged + staged changes) or `git diff main...HEAD` (branch vs main) to enumerate changed files.
- Exclude generated files (`target/`, `$OUT_DIR`, `font_unifont*.bin`) and vendored third-party code.

## 2. Run automated checks first

```bash
cargo test 2>&1          # All unit + integration tests must pass
cargo clippy -- -D warnings 2>&1   # Zero warnings required
```

Report any failures immediately. Do not proceed to manual review until both are clean.

## 3. Correctness review

For each changed source file, check:

- **OS/2 API behaviour** — Verify implementations against the IBM *Control Program Programming Reference*, OS/2 Warp 4 Toolkit headers, or the existing `doc/` documentation. Check that error codes match the OS/2 spec (e.g., `ERROR_INVALID_HANDLE = 6`, not a Linux errno).
- **Return-value conventions** — OS/2 APIs return `APIRET` (u32) in RAX. Confirm the correct value is placed in `regs.rax` on success and on each error path.
- **Ordinal registration** — Any new API must be added to both `targets/os2api.def` *and* `src/loader/api_registry.rs`. Run `cargo run --bin gen_api -- check` and confirm it reports clean.
- **Sub-dispatcher routing** — APIs dispatched via subsystem bases (KBDCALLS_BASE, VIOCALLS_BASE, etc.) must use the correct ordinal offset.
- **Guest memory access** — All reads/writes through `guest_mem` helpers must use the bounds-checked API; no raw pointer arithmetic from guest-supplied values.
- **String handling** — Guest strings must be decoded through `read_guest_string` (which calls `cp_decode` with the active codepage). Direct `from_utf8_lossy` on raw guest bytes is wrong.
- **Handle lifecycle** — Confirm handles are allocated, reference-counted, and freed correctly. Watch for leaked semaphores, DLL refcount mismatches, or un-freed guest allocations in error paths.

## 4. Security review

Warpine's threat model is hypervisor escape from a malicious OS/2 guest. Check every changed `unsafe` block and every API handler:

- **Bounds checking** — Guest-supplied addresses (registers, stack args, pointer fields in structs) must be clamped/validated against the 128 MB guest window before any host dereference. A guest address must never become a raw host pointer without going through `guest_mem`.
- **Length limits** — Buffers or counts read from guest memory must be capped before allocation or copy. Unbounded `Vec::with_capacity(guest_len)` from an untrusted length is a DoS / OOM vector.
- **Path traversal** — OS/2 paths forwarded to the host VFS must be sanitised: no `../` traversal outside the mapped drive root, no absolute host paths derived from guest input.
- **Ordinal range checks** — Array indexing by ordinal must be range-checked; a guest-triggered out-of-bounds lookup must return an error, not panic.
- **KVM ioctl errors** — All KVM ioctl return values must be checked. Silent `unwrap()` on a KVM result is a crash vector.
- **Privilege level** — `MAGIC_API_BASE` INT 3 breakpoints must only be honoured when the guest is in ring 3 (CPL=3). Check that `vcpu.rs` verifies CPL before dispatching.

Flag any finding with a `[SECURITY]` prefix and its severity (Critical / High / Medium / Low).

## 5. Test coverage

- Every new API entry point must have at least one unit test verifying the success path and one verifying the primary error path.
- New data-structure helpers (managers, parsers) must have unit tests in the same file.
- If a sample OS/2 app would be the natural oracle, note that one should be added to `samples/`.
- Check that test names are descriptive (`test_dos_alloc_mem_exceeds_limit`, not `test1`).
- Confirm tests do not use `unwrap()` on results that could legitimately fail — use `expect()` with a message or proper assertions.

## 6. Clean-room compliance

- No IBM-proprietary binary blobs, ROM dumps, or disassembly artefacts must be introduced.
- Run: `file vendor/**/* samples/**/* 2>/dev/null | grep -vE 'text|directory|makefile|script|source|ELF|PE32|data'` and flag anything unexpected.
- Confirm all new behaviour is derived from public documentation or observable behaviour of Open Watcom-compiled apps.

## 7. Code style and conventions

- Named constants only — no magic numbers for GDT selectors, API ordinals, memory bases, or error codes. Check `constants.rs` for existing definitions before adding new ones.
- `unsafe` blocks must have a `// SAFETY:` comment explaining the invariant being upheld.
- Stub functions must follow the stubbing pattern: log a `[STUB]` message and return a reasonable error code.
- No `println!` in production paths — use the structured logging/trace macros already in the codebase.
- Module separation: API implementations belong in their domain module (`doscalls.rs`, `pm_win.rs`, etc.), not in `mod.rs` or `vcpu.rs`.

## 8. Code smells

Look for structural problems that are not bugs today but become bugs or maintenance burdens tomorrow. Flag each with a `[SMELL]` prefix.

**Dead and redundant code**
- Functions, constants, or `use` imports that are never referenced — prefer outright deletion over `#[allow(dead_code)]`.
- Duplicate logic that should be a shared helper (same pattern appearing 3+ times across the file).
- `#[allow(...)]` suppressions that paper over a real problem; prefer fixing the root cause.

**Complexity and readability**
- Functions longer than ~60 lines or with a nesting depth > 3 — candidates for extraction.
- Match arms or if-chains that repeat the same sub-expression — factor out a let-binding or helper.
- Boolean parameters that flip behaviour (`do_thing(true)`) — prefer an enum or two named functions.
- Reversed/misleading variable names (e.g., `is_not_valid` where `is_invalid` would be clearer).

**Error handling**
- `unwrap()` / `expect()` on values that originate from guest input or external I/O — must use `?` or explicit error handling.
- Silently swallowing errors with `let _ = ...` or an empty `Err(_) => {}` arm when the failure is meaningful.
- Mixing OS/2 error codes (`APIRET`) with Rust `Result` without a clear conversion boundary.

**Warpine-specific smells**
- Lock acquisition order — always acquire `SharedState` sub-locks in the documented order (memory → handles → dll → window → socket) to prevent deadlock; flag any deviation.
- Holding a mutex across a blocking operation (I/O, sleep, condvar wait) — should release the lock first.
- Guest memory pointer stored in a host `struct` field beyond the duration of a single API call — a stored raw guest address is stale after any guest reallocation.
- `Arc::clone` on a manager inside a hot VMEXIT path without a clear need — prefer passing a borrow.
- API handler that calls another API handler directly (bypasses the dispatch table, breaks tracing and ring-buffer recording).
- Hardcoded English strings in API responses that should route through the codepage/NLS layer.
- `regs.rax` written more than once in a single handler — only the last write counts; earlier ones are dead.

**Abstraction and coupling**
- `mod.rs` / `vcpu.rs` growing new business logic — it belongs in a domain module.
- A module importing from a sibling's private submodule via `super::super::` path — signals missing abstraction boundary.
- `SharedState` fields added without a corresponding `Arc<Mutex<...>>` wrapper when the field is accessed from multiple threads.

## 10. Documentation hygiene

- `CLAUDE.md` — Update the API entry point count and any architecture notes that changed.
- `doc/TODOs.md` — Remove completed items; add any newly discovered gaps.
- `CHANGELOG.md` — Confirm there is an entry for the change under `[Unreleased]`.
- `doc/developer_guide.md` — Update test counts if new tests were added.
- Inline comments — Ensure non-obvious logic is explained; remove stale comments that no longer match the code.

## 11. Report

Present findings as a structured list grouped by category. For each finding include:
- **File and line** (as a clickable markdown link)
- **Severity** for security items: `[SECURITY: Critical/High/Medium/Low]`
- **Description** — what is wrong or could be improved
- **Suggested fix** — concrete, actionable

Conclude with a summary table:

| Category | Issues Found |
|---|---|
| Correctness | N |
| Security | N |
| Test coverage | N |
| Clean-room | N |
| Style | N |
| Code smells | N |
| Documentation | N |

If no issues are found in a category, write `✓ Clean`.

Do **not** make any edits during the review pass. Present the full report first and wait for the user to confirm which findings to act on.
