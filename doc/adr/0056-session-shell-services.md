# ADR-0056: The desktop server keeps the session's switch list and system atom table; DDE crosses programs

- **Status:** Accepted; supersedes ADR-0030; implements ADR-0052's phase C except drag and drop between programs
- **Date:** 2026-11-19
- **Sources:**
  - The session's tables: `src/desktop/services.rs`, `src/gui/desktop_server.rs`
  - Protocol: `src/desktop/proto.rs` (`SwitchEntry`; `WinCall::Switch*`, `Atom*`, `DdeTargets`; `WinReply::Error`, `Text`, `Switches`, `Windows`; `DesktopLink::notify`)
  - Programs: `src/loader/pm_switch.rs` (session routing, window list), `src/loader/pm_atom.rs`, `src/loader/pm_dde.rs`, `src/loader/pm_remote.rs` (`remote_batch`), `src/loader/pm_dialog.rs` (`SYSTEM_ACCELS`)
  - Sample `samples/pm_xshell_test`
  - Earlier decisions: ADR-0030, ADR-0049, ADR-0052–0055
  - IBM *PM Reference*: switch list functions, atom functions, `WinDdeInitiate`, `WinDdeRespond`, `WinDdePostMsg`, `WM_SYSCOMMAND` (`SC_TASKMANAGER`)

## Context

ADR-0030 kept the switch list, the system atom table and DDE within one Warpine process because the processes shared no desktop. Since ADR-0052 they do, and phase B lets a program call, send to and list the windows of every other program.

On OS/2 these services are system-wide:

- the window list (Ctrl+Esc) shows every program;
- the system atom table is shared;
- DDE reaches any program.

Each program is still its own process with its own memory, so the shared tables must live somewhere every program can reach. The data passed through DDE must also be memory both sides can read.

## Decision

### The server keeps the tables

- **Where they live.** The desktop server, which outlives the programs of the session, keeps the session's switch list and system atom table (`desktop::services::Services`). Programs reach them with calls at `SERVER_ADDRESS`, answered by the server at once. A failure comes back as `WinReply::Error(PMERR_*)`.
- **Switch list.**
  - Every program sees every entry.
  - An entry belongs to the program that added it: only that program changes or removes it, and its entries go when it leaves.
  - Titles travel as text, decoded from and encoded to each program's codepage.
  - A `FCF_TASKLIST` frame's entry is added, and destroyed windows forgotten, with `DesktopLink::notify`, a call nobody waits for. The window manager can therefore tell the server under its lock.
  - At most 256 entries in all and 64 per program.
- **System atom table.**
  - Names are text, compared without regard to ASCII case.
  - Atoms stay when the program that added them ends.
  - Tables made with `WinCreateAtomTable` stay the process's.
- **In-process desktop.** With the desktop in the process (`WARPINE_DESKTOP=local`, no session) each process keeps its own tables, as before.

### The window list

- **Opening it.** Ctrl+Esc is a system accelerator for `WM_SYSCOMMAND(SC_TASKMANAGER)`. `WinDefWindowProc` and `WinDefDlgProc` show the visible switch-list entries in a pop-up menu in the middle of the desktop, using the program's own pop-up machinery.
- **Picking an item.** Each item is `MIS_SYSCOMMAND` with one of Warpine's own codes, `SC_WINDOWLIST_FIRST` (0x8F00) + n, above PM's `SC_*` range. Default processing of that code switches to the entry: `WinSwitchToProgram`, through the server for another program's frame.

### DDE between programs

- **Initiating.** `WinDdeInitiate` sends `WM_DDE_INITIATE` first to the clients of every other program's frames, which the `DdeTargets` call lists with the program's process id. Then it sends to the program's own frames.
  - Each remote send is waited for in a `remote_batch`: ADR-0055's wait generalized to a list of calls. So a server's `WinDdeRespond` reaches the client before `WinDdeInitiate` returns, as within a program.
  - A batch survives the thread running messages sent to it. The API call is made again and goes on with the same batch (`remote_resuming` keeps it from preparing the calls twice).
- **Shared structures.** The `DDEINIT` for another program is a block of the session's shared arena (ADR-0049), given to the receiving process, which frees it. The sender lets go of it at once. `WinDdeRespond` to another program's client sends `WM_DDE_INITIATEACK` the same way.
- **Posting.** `WinDdePostMsg` to another program's window takes a `DDESTRUCT` in the shared arena (`DosAllocSharedMem` giveable), gives it to the receiving process, posts the message, and lets the sender's hold go unless `DDEPM_NOFREE`.

### Warpine's choices (unverified against OS/2)

- **Entry ownership.** Only an entry's own program may change or remove it.
- **Limits.** The switch list holds 256 entries in all and 64 per program.
- **Window list.** It is a pop-up menu listing visible entries; there is no separate window. Its items use Warpine's own `SC_*` codes.
- **Private `DDESTRUCT`s.** A `DDESTRUCT` outside shared memory posted to another program fails (`PMERR_INVALID_PARM`).
- **DDE send hooks.** As for any send between programs (ADR-0055), the receiving thread runs its queue's send hooks.

## Consequences

- **What works.** The PM programs of a session share the window list, the switch list and system atoms, and hold DDE conversations with each other. `pm_xshell_test` checks all of these across a parent and a child, with the window list driven by Ctrl+Esc.
- **Lifetime.** The tables live as long as the desktop server, which is as long as the session has a PM program, plus its linger.
- **Costs.** Each switch-list and system-atom call is a round trip to the server. A DDE initiation costs one call per other program plus a waited send per window that takes DDE.
- **Still missing.** Drag and drop between programs, the rest of ADR-0052's phase C: `DrgDrag` over another program's windows and `DRAGINFO` in shared memory.
