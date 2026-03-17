// SPDX-License-Identifier: GPL-3.0-only
//
// End-to-end integration tests: run OS/2 sample binaries, capture stdout
// and exit code, and assert against known-good output.
//
// Requires /dev/kvm.  Tests are skipped (pass silently) when KVM is absent,
// so the suite stays green in restricted CI environments.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Return the path to the compiled warpine binary.  Cargo sets
/// CARGO_BIN_EXE_warpine automatically when building integration tests.
fn warpine() -> &'static str {
    env!("CARGO_BIN_EXE_warpine")
}

/// Return `true` if KVM is accessible on this machine.
fn kvm_available() -> bool {
    Path::new("/dev/kvm").exists()
}

/// Run `sample` (path relative to workspace root) with WARPINE_HEADLESS=1,
/// wait up to `timeout` seconds, and return `(stdout, exit_code)`.
///
/// Panics if the process cannot be spawned or times out.
fn run_sample(sample: &str, timeout_secs: u64) -> (String, i32) {
    let mut child = Command::new(warpine())
        .arg(sample)
        .env("WARPINE_HEADLESS", "1")
        // Suppress tracing/log noise on stderr; we only care about stdout.
        .env("RUST_LOG", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn warpine for {sample}: {e}"));

    // Poll until the process exits or the deadline passes.
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait().expect("try_wait failed") {
            Some(status) => {
                let out = child.stdout.take().unwrap();
                use std::io::Read;
                let mut buf = String::new();
                // stdout handle is a pipe; read it to EOF
                std::io::BufReader::new(out).read_to_string(&mut buf).ok();
                let code = status.code().unwrap_or(-1);
                return (buf, code);
            }
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    panic!("{sample} did not complete within {timeout_secs}s");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// hello: DosWrite to stdout, clean exit.
#[test]
fn test_hello_world() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/hello/hello.exe", 15);
    assert_eq!(code, 0, "hello exited with {code}\nstdout:\n{out}");
    assert!(
        out.contains("Hello from Warpine OS/2 Environment!"),
        "unexpected output:\n{out}"
    );
}

/// alloc_test: DosAllocMem / DosFreeMem round-trip.
#[test]
fn test_alloc_test() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/alloc_test/alloc_test.exe", 15);
    assert_eq!(code, 0, "alloc_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("Alloc succeeded"),        "missing 'Alloc succeeded':\n{out}");
    assert!(out.contains("Data in allocated memory"), "missing data check:\n{out}");
    assert!(out.contains("Free succeeded"),          "missing 'Free succeeded':\n{out}");
}

/// nls_test: DosQueryCp, DosQueryCtryInfo, DosGetDateTime, DosMapCase.
#[test]
fn test_nls() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/nls_test/nls_test.exe", 15);
    assert_eq!(code, 0, "nls_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("All tests PASSED!"), "NLS tests failed:\n{out}");
    // Spot-check individual API results
    assert!(out.contains("DosQueryCp returns 0 OK"),       "DosQueryCp:\n{out}");
    assert!(out.contains("DosQueryCtryInfo returns 0 OK"), "DosQueryCtryInfo:\n{out}");
    assert!(out.contains("DosMapCase converts to HELLO OK"), "DosMapCase:\n{out}");
}

/// thread_test: DosCreateThread / DosWaitThread.
#[test]
fn test_thread() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/thread_test/thread_test.exe", 15);
    assert_eq!(code, 0, "thread_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("Hello from child thread!"), "thread output:\n{out}");
}

/// pipe_test: DosCreatePipe, DosWrite, DosRead.
#[test]
fn test_pipe() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/pipe_test/pipe_test.exe", 15);
    assert_eq!(code, 0, "pipe_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("DosCreatePipe rc=0"), "pipe create:\n{out}");
    assert!(out.contains("Read from pipe: 'Pipe Test Data'"), "pipe read:\n{out}");
}

/// mutex_test: DosCreateMutexSem, recursive locking, inter-thread handoff.
#[test]
fn test_mutex() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/mutex_test/mutex_test.exe", 15);
    assert_eq!(code, 0, "mutex_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("Recursive locks OK"), "recursive lock:\n{out}");
    assert!(out.contains("Child: Got mutex!"),  "child lock:\n{out}");
}

/// queue_test: DosCreateQueue, DosWriteQueue, DosReadQueue.
#[test]
fn test_queue() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/queue_test/queue_test.exe", 15);
    assert_eq!(code, 0, "queue_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("DosCreateQueue rc=0"), "queue create:\n{out}");
    assert!(out.contains("DosReadQueue rc=0"),   "queue read:\n{out}");
    assert!(out.contains("msg='Queue Message'"), "queue message:\n{out}");
}

/// thunk_test: TIB/PIB layout, DosGetInfoBlocks, DosQuerySysInfo.
#[test]
fn test_thunk() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/thunk_test/thunk_test.exe", 15);
    assert_eq!(code, 0, "thunk_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("All tests PASSED!"), "thunk tests failed:\n{out}");
    assert!(out.contains("DosGetInfoBlocks returns 0 OK"), "DosGetInfoBlocks:\n{out}");
    assert!(out.contains("DosQuerySysInfo returns 0 OK"),  "DosQuerySysInfo:\n{out}");
}
