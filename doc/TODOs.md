# Warpine TODO List

This document tracks the tasks required to reach a functional OS/2 compatibility layer.

## Phase 1: Foundation (CLI "Hello World")
- [ ] **Executable Parser (LX/LE/NE)**
    - [ ] Implement MZ (DOS) header parser to locate the OS/2 header offset.
    - [ ] Implement LX (Linear Executable) header parser.
    - [ ] Implement Object Table and Page Map parsing for LX files.
    - [ ] Implement Fixup (Relocation) Table parsing.
- [ ] **Loader Subsystem**
    - [ ] Implement memory mapping of LX objects into the process address space.
    - [ ] Apply base relocations (fixups).
    - [ ] Resolve dynamic imports (DLLs) and thunk them to native implementations.
    - [ ] Set up the initial CPU state (registers, stack) for jumping into OS/2 entry point.
- [ ] **Initial API Thunks (DOSCALLS.DLL)**
    - [ ] `DosWrite`: Basic implementation for stdout/stderr.
    - [ ] `DosExit`: Proper process termination with exit code.
    - [ ] `DosPutMessage`: Simple message output to console.

## Phase 2: Core OS/2 Subsystem
- [ ] **Memory Management**
    - [ ] `DosAllocMem` / `DosFreeMem` implementation.
    - [ ] Handle OS/2 32-bit flat memory model vs. segmented requests.
- [ ] **Filesystem APIs**
    - [ ] `DosOpen`, `DosRead`, `DosClose`, `DosQueryFileInfo`.
    - [ ] Map OS/2 drive letters (e.g., `C:\`) to Unix paths.
- [ ] **Process/Thread Management**
    - [ ] `DosCreateThread`, `DosKillThread`.
    - [ ] Thread Local Storage (TLS) emulation.
- [ ] **Inter-Process Communication (IPC)**
    - [ ] Semaphores (`DosCreateEventSem`, `DosPostEventSem`).
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
- [ ] Create a sample 32-bit OS/2 "Hello World" binary for testing.
