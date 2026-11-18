# ADR-0022: Carry named pipes and inherited handles over host sockets and the host exec

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** commit ebddef5; `src/loader/npipe.rs`, `src/loader/npipe_api.rs`, `src/loader/inherit.rs`, `src/loader/managers.rs` (handle table); IBM *Control Program Programming Reference* (`DosCreateNPipe`, `DosExecPgm`, `RESULTCODES`); ADR-0020

## Context

OS/2 programs talk to each other and to their children through pipes: anonymous pipes inherited by a child (`DosCreatePipe` + `DosExecPgm`, how a shell builds `a | b`), and named pipes (`\PIPE\name`) with byte or message modes, several instances, blocking and non-blocking ends, `DosTransactNPipe` and `DosCallNPipe`. A child also gets its parent's argument block and environment.

Each OS/2 process is a separate Warpine host process (ADR-0020), so none of this can live in one process's memory. Before this decision a child got neither its command line, its environment nor its pipe handles, and 4OS2 pipelines hung. Pipe handles were numbered from 0x1000, apart from file handles, which broke Open Watcom C runtimes that index a small table by handle.

## Decision

- **Named pipes are host `AF_UNIX` stream socket pairs.** Each connection carries frames (`[kind u8][len u32 LE][payload]`): one per message of a message pipe, chunks of a byte pipe, or the server's disconnect. Each end drains its socket into a local queue, which gives message reads, `ERROR_MORE_DATA` remainders and `DosPeekNPipe`.
- **Pipe names live in a private session directory** (mode 0700, named to children by `WARPINE_PIPE_DIR`): the server process listens on a socket file named after a hash of the pipe name, and an accept thread hands each client to a listening instance. Clients in the same Warpine process and in its child processes reach a pipe the same way; other host processes and other Warpine sessions cannot see it.
- **A child's inheritance goes through the host `exec`** (`inherit.rs`): `WARPINE_OS2ARG0` / `WARPINE_OS2ARGS` (the argument block, escaped), `WARPINE_OS2ENV` (the environment block) and `WARPINE_HANDLES` (inheritable pipe ends with their numbers and states). The host descriptors stay close-on-exec in the parent; `pre_exec` clears the flag only in the forked child, so no other process, not even another child started at the same time, receives them. The server end of a named pipe stays with its creator.
- **Files and pipes share one handle space**, lowest free number first, as on OS/2.
- `EXEC_ASYNC` / `EXEC_ASYNCRESULT` return the child's process id in `RESULTCODES.codeTerminate`, as the CP Reference says.

## Consequences

- Shell pipelines work across Warpine processes (an integration test runs a 4OS2 pipeline), and the host kernel does the buffering, blocking and end-of-pipe detection.
- No guest memory or host memory is shared between processes; all crossings are explicit, bounded (message size, queue size, name length, environment and argument sizes, handle count) and checked on arrival.
- Named pipes are visible only inside one Warpine session (the starting process and its descendants), not to unrelated host processes or other sessions; OS/2 networks' remote pipes (`\\server\PIPE\…`) are not supported.
- A killed Warpine process can leave its session directory behind; cleaning stale directories is a follow-up (`doc/TODOs.md`).
- Behaviour the references leave open (message header size in `DosPeekNPipe`, whether a child may serve its parent's instance) is marked unverified in the code.
- Named shared memory between processes (`\SHAREMEM\`) still needs its own host mechanism; this decision does not cover it.
