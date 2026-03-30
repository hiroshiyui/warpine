---
name: security-audit
description: Perform project-wide security audits focused on KVM hypervisor escape, guest memory safety, and dependency vulnerabilities.
---

When performing a security audit, always follow these steps:

1. **Audit dependencies** — run `cargo audit` to check for known CVEs in the dependency tree. Treat any `Critical` or `High` finding as a blocker; document `Medium`/`Low` with remediation notes.

2. **Static analysis** — run `cargo clippy -- -W clippy::all -W clippy::pedantic 2>&1` and review all warnings. Pay particular attention to:
   - Integer overflow/underflow in address arithmetic
   - Unchecked slice indexing into guest memory buffers
   - `unwrap()`/`expect()` on values derived from untrusted guest input

3. **Hypervisor escape review** — read every `unsafe` block in `src/loader/` and verify:
   - All guest-to-host pointer translations go through the bounds-checked guest memory window (128 MB). A guest-supplied address must never be used as a raw host pointer without clamping.
   - KVM `ioctl` return values are always checked; no silent error suppression.
   - GDT/LDT descriptor construction does not allow the guest to craft selectors pointing outside the guest memory region.
   - `MAGIC_API_BASE` breakpoints cannot be triggered from an unexpected privilege level to escalate host privileges.

4. **Guest-controlled input audit** — trace all data paths from VMEXIT guest registers (RAX, RBX, RCX, RDX, RSI, RDI, guest stack) into host Rust code. Confirm that:
   - String/buffer lengths read from guest memory are capped before allocation.
   - OS/2 path strings are sanitised (no `../` traversal) before being forwarded to the host VFS.
   - Ordinal numbers from the API dispatch table are range-checked before array indexing.

5. **Clean-room policy check** — confirm no proprietary binary blobs (IBM DLL dumps, ROM images, disassembly artefacts) have been introduced into `vendor/`, `samples/`, or anywhere else in the tree. Run `file vendor/**/* samples/**/* 2>/dev/null | grep -vE 'text|directory|makefile|script|source'` and review anything unexpected.

6. **Report findings** — document all identified risks, classified by severity (Critical, High, Medium, Low), with:
   - Affected file and line number
   - Description of the vulnerability or policy violation
   - Specific remediation steps

   Present the report to the user before making any changes. Fix Critical and High issues immediately; schedule Medium/Low for follow-up.
