# Warpine TODO List

This document tracks the tasks required to reach a functional OS/2 compatibility layer.

## Phase 1: Foundation (CLI "Hello World") - COMPLETED 🎉
- [x] **Executable Parser (LX/LE/NE)**
    - [x] Implement MZ (DOS) header parser to locate the OS/2 header offset.
    - [x] Implement LX (Linear Executable) header parser.
    - [x] Implement Object Table and Page Map parsing for LX files.
    - [x] Implement Fixup (Relocation) Table parsing.
- [x] **Loader Subsystem**
    - [x] Implement memory mapping of LX objects into the KVM guest address space.
    - [x] Apply base relocations (fixups).
    - [x] Resolve dynamic imports (DLLs) and thunk them to native implementations via INT 3 traps.
    - [x] Set up the initial CPU state (registers, stack, TIB, PIB, Env) for execution.
- [x] **Initial API Thunks (DOSCALLS.DLL)**
    - [x] `DosWrite`: Basic implementation for stdout/stderr.
    - [x] `DosExit`: Proper process termination with exit code.
    - [x] `DosQuerySysInfo`: Stub implementation for runtime initialization.
    - [x] `DosQueryConfig`: Stub implementation for runtime initialization.
    - [x] `DosQueryHType`: Identification of standard handles.
    - [x] `DosGetInfoBlocks`: Retrieval of TIB and PIB pointers.

## Phase 2: Core OS/2 Subsystem
- [x] **Memory Management**
    - [x] `DosAllocMem` / `DosFreeMem` implementation.
    - [x] Handle OS/2 32-bit flat memory model vs. segmented requests.
- [x] **Filesystem APIs**
    - [x] `DosOpen`, `DosRead`, `DosClose`, `DosQueryFileInfo`.
    - [x] `DosDelete`, `DosMove`, `DosCreateDir`, `DosDeleteDir`.
    - [x] `DosFindFirst`, `DosFindNext` with basic wildcard support.
    - [x] Map OS/2 drive letters (e.g., `C:\`) to Unix paths.
- [x] **Process/Thread Management**
    - [x] `DosCreateThread`, `DosKillThread`.
    - [x] Thread Local Storage (TLS) emulation (via TIB initialization).
- [x] Inter-Process Communication (IPC)
    - [x] Event Semaphores (`DosCreateEventSem`, `DosPostEventSem`, etc.).
    - [x] Mutex Semaphores (`DosCreateMutexSem`, `DosRequestMutexSem`, etc.).
    - [x] MuxWait Semaphores (`DosCreateMuxWaitSem`, `DosWaitMuxWaitSem`).
    - [ ] Pipes and Queues.


## Phase 3: Presentation Manager (GUI)
- [ ] **Window Management (PMWIN.DLL)**
    - [ ] Implement message queue (`WinCreateMsgQueue`).
    - [ ] Window creation and event loop mapping to X11/Wayland.
- [ ] **Graphics (PMGPI.DLL)**
    - [ ] Map Gpi drawing functions (lines, boxes, text) to Cairo or Skia.
- [ ] **Input Handling**
    - [ ] Translate Unix mouse/keyboard events to OS/2 `WM_` messages.

## Phase 4: Multimedia and 16-bit Support
- [ ] **Audio/Video (MMPM2)**
    - [ ] Reimplement multimedia APIs using PulseAudio/ALSA or SDL.
- [ ] **16-bit Compatibility**
    - [ ] Integrate a lightweight x86 emulator for 16-bit code execution.
    - [ ] Support NE (New Executable) format parsing and loading.

## General Improvements
- [ ] Add unit tests for LX parser and API stubs.
- [ ] Improve error handling and logging (possibly using `log` or `tracing` crates).
- [x] Create a sample 32-bit OS/2 "Hello World" binary for testing.
- [x] Pivot to Unicorn Engine for platform-agnostic 32-bit emulation.
