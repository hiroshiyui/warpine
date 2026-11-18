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
/// Country and codepage for every run: Warpine takes them from the host
/// locale, so tests pin the US defaults to stay reproducible on any machine.
const PINNED_LOCALE: &[(&str, &str)] = &[("WARPINE_COUNTRY", "1"), ("WARPINE_CODEPAGE", "437")];

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
    run_sample_env(sample, timeout_secs, &[])
}

/// [`run_sample`] with extra environment variables (applied after the
/// pinned locale, so they can override it).
fn run_sample_env(sample: &str, timeout_secs: u64, env: &[(&str, &str)]) -> (String, i32) {
    run_sample_args(sample, &[], timeout_secs, env)
}

/// [`run_sample_env`] with command-line arguments for the program.
fn run_sample_args(sample: &str, args: &[&str], timeout_secs: u64, env: &[(&str, &str)]) -> (String, i32) {
    // Drive C: is $XDG_DATA_HOME/warpine/drive_c: a fresh one per run, never
    // the user's own (`env` may still name another).
    let data_home = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(warpine())
        .arg(sample)
        .args(args)
        .env("WARPINE_HEADLESS", "1")
        .env("XDG_DATA_HOME", data_home.path())
        // Named-pipe and thread-session directories go there too, never
        // into the user's own runtime directory.
        .env("XDG_RUNTIME_DIR", data_home.path())
        .envs(PINNED_LOCALE.iter().copied())
        .envs(env.iter().copied())
        // Suppress tracing/log noise on stderr; we only care about stdout.
        .env("RUST_LOG", "error")
        // End of input at once: a sample that waits for a key (audio_test's
        // "Press ENTER") must not block on the test runner's own stdin.
        .stdin(Stdio::null())
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
    // NLS ordinals 6 and 8 (<bseord.h>): DosQueryDBCSEnv used to be answered
    // with country information.
    assert!(out.contains("CP932 lead bytes 81-9F, E0-FC, then 0,0 OK"), "DosQueryDBCSEnv:\n{out}");
    assert!(out.contains("short buffer: ERROR_NLS_TABLE_TRUNCATED OK"), "truncation:\n{out}");
    assert!(out.contains("'a' and 'A' weigh the same OK"), "DosQueryCollate:\n{out}");
}

/// The country and starting codepage come from WARPINE_COUNTRY /
/// WARPINE_CODEPAGE (else the host locale): a non-US country and a DBCS
/// codepage reach DosQueryCp / DosQueryCtryInfo, and the Watcom C runtime
/// starts normally with them (its start-up DosQueryDBCSEnv call used to
/// hang it for every non-US country).
#[test]
fn test_country_and_codepage_from_the_environment() {
    if !kvm_available() { return; }
    let (out, code) = run_sample_env("samples/nls_test/nls_test.exe", 15,
                                     &[("WARPINE_COUNTRY", "81"), ("WARPINE_CODEPAGE", "932")]);
    assert_eq!(code, 0, "nls_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("codepage[0]=932"), "DosQueryCp:\n{out}");
    assert!(out.contains("country=81"), "DosQueryCtryInfo country:\n{out}");
    assert!(out.contains("All tests PASSED!"), "NLS tests failed:\n{out}");
}

/// dbcs_test: DosQueryDBCSEnv after DosSetProcessCp(932) (NLS 6), then
/// VioWrtCellStr (VIOCALLS 10) of a Shift-JIS pair and VioCheckCharType
/// (VIOCALLS 75, os2386.lib VIO16CHECKCHARTYPE) classifying its cells.
#[test]
fn test_dbcs_env_and_vio_char_types() {
    if !kvm_available() || !Path::new("samples/dbcs_test/dbcs_test.exe").exists() { return; }
    let (out, code) = run_sample("samples/dbcs_test/dbcs_test.exe", 15);
    assert!(out.contains("All tests PASSED!"), "dbcs_test failed (exit {code}):\n{out}");
    for line in ["CP932 range (0x81,0x9F) present OK",
                 "col 0 classified as DBCS-lead (type==2) OK",
                 "col 1 classified as DBCS-trail (type==3) OK"] {
        assert!(out.contains(line), "missing '{line}':\n{out}");
    }
}

/// ansi_test: ANSI escape sequences written with VioWrtTTY and DosWrite,
/// read back with VioReadCellStr / VioGetCurPos. Headless with a piped
/// standard output the bytes reach the pipe unchanged, and the VIO screen
/// still follows them.
#[test]
fn test_ansi_escape_sequences_headless() {
    let sample = "samples/ansi_test/ansi_test.exe";
    if !kvm_available() || !Path::new(sample).exists() { return; }
    let (out, code) = run_sample(sample, 15);
    let failures: Vec<&str> = out.lines().filter(|l| l.contains("FAIL")).collect();
    assert!(failures.is_empty(), "failures: {failures:?}\n{out}");
    assert!(out.contains("done: 0 failed"), "incomplete (exit {code}):\n{out}");
    assert!(out.contains("ok ESC[2J clears the screen") && out.contains("ok a sequence split across writes"),
            "checks missing:\n{out}");
    assert!(out.contains("\x1b[2J"), "the escape bytes reach the pipe as they are");
    assert_eq!(code, 0);
}

/// ansi_test in a VIO window (a pseudo-terminal; WARPINE_GUI_SCRIPT only
/// ends the session): the same checks on the screen the window shows. The
/// results are read from C:\ANSI_TEST.OUT.
#[test]
fn test_ansi_escape_sequences_in_a_vio_window() {
    let sample = "samples/ansi_test/ansi_test.exe";
    let has_script = Command::new("script").arg("--version").output().is_ok_and(|o| o.status.success());
    if !kvm_available() || !Path::new(sample).exists() || !has_script { return; }
    let dir = tempfile::tempdir().expect("tempdir");
    let session = dir.path().join("session.script");
    std::fs::write(&session, "5000 quit\n").unwrap();
    let status = Command::new("script")
        .args(["-qec", &format!("timeout 30 {} {sample}", warpine())])
        .arg("/dev/null")
        .envs(PINNED_LOCALE.iter().copied())
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("RUST_LOG", "error")
        .env("XDG_DATA_HOME", dir.path())
        // Named-pipe and thread-session directories go there too, never
        // into the user's own runtime directory.
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("WARPINE_GUI_SCRIPT", &session)
        .env_remove("WARPINE_VIO")
        .env_remove("WARPINE_HEADLESS")
        .stdout(Stdio::null())
        .status()
        .expect("run script");
    assert!(status.success(), "ansi_test session failed: {status}");
    let drive_c = dir.path().join("warpine/drive_c");
    let file = std::fs::read_dir(&drive_c).expect("no C: drive").flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("ANSI_TEST.OUT"))
        .expect("ansi_test wrote no result file");
    let out = std::fs::read_to_string(file.path()).unwrap();
    let failures: Vec<&str> = out.lines().filter(|l| l.contains("FAIL")).collect();
    assert!(failures.is_empty(), "failures: {failures:?}\n{out}");
    assert!(out.contains("done: 0 failed"), "incomplete:\n{out}");
}

/// 4OS2 reading commands from a pipe: once the input is used up, its
/// waiting KbdCharIn ends the process (exit code 0) instead of polling
/// forever.
#[test]
fn test_4os2_ends_at_the_end_of_piped_input() {
    let sample = "samples/4os2/4os2.exe";
    if !kvm_available() || !Path::new(sample).exists() { return; }
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(warpine())
        .arg(sample)
        .envs(PINNED_LOCALE.iter().copied())
        .env("RUST_LOG", "error")
        .env("XDG_DATA_HOME", dir.path())
        // Named-pipe and thread-session directories go there too, never
        // into the user's own runtime directory.
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env_remove("WARPINE_VIO")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn warpine");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"echo piped-Qz7\r\necho saved > out.txt\r\n").unwrap();
    } // closed: end of input
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() { break s; }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("4OS2 kept waiting for keys after the end of its piped input");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut out = String::new();
    use std::io::Read;
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert_eq!(status.code(), Some(0), "exit status at the end of input:\n{out}");
    assert!(out.contains("piped-Qz7"), "the piped command ran:\n{out}");
    let saved = std::fs::read_to_string(dir.path().join("warpine/drive_c/out.txt")).unwrap_or_default();
    assert_eq!(saved, "saved\r\n", "echo > file wrote the file");
}

/// 4OS2 pipes to an external program: `ver | find "…"` creates a pipe,
/// starts FIND.EXE (npipe_test.exe under that name) with the pipe as its
/// standard input and the write end not inherited, runs VER into the pipe
/// and waits for FIND, which prints only the matching line.
#[test]
fn test_4os2_pipe_to_an_external_program() {
    let sample = "samples/4os2/4os2.exe";
    let find = "samples/npipe_test/npipe_test.exe";
    if !kvm_available() || !Path::new(sample).exists() || !Path::new(find).exists() { return; }
    let dir = tempfile::tempdir().expect("tempdir");
    let drive_c = dir.path().join("warpine/drive_c");
    std::fs::create_dir_all(&drive_c).unwrap();
    std::fs::copy(find, drive_c.join("FIND.EXE")).unwrap();
    let mut child = Command::new(warpine())
        .arg(sample)
        .envs(PINNED_LOCALE.iter().copied())
        .env("RUST_LOG", "error")
        .env("XDG_DATA_HOME", dir.path())
        // Named-pipe and thread-session directories go there too, never
        // into the user's own runtime directory.
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env_remove("WARPINE_VIO")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn warpine");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"ver | find \"NoSuchTextZq\"\r\nver | find \"OS/2 Version\"\r\necho pipes-done\r\n").unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() { break s; }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("4OS2 hung in a pipeline");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut out = String::new();
    use std::io::Read;
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert_eq!(status.code(), Some(0), "{out}");
    assert!(out.contains("pipes-done"), "4OS2 went on after the pipelines:\n{out}");
    // VER's own line (two spaces after 4OS2, unlike the start-up banner)
    // comes out once, through FIND; its copyright lines are filtered.
    assert_eq!(out.matches("4OS2  5.00").count(), 1, "VER's line through FIND:\n{out}");
    assert_eq!(out.matches("Copyright (c) 1988").count(), 1, "only the banner's copyright:\n{out}");
}

/// audio_test: DosBeep and mciSendString through MDM loaded at run time
/// (`open waveaudio alias …` opens the device by name). Runs headless, so
/// no sound is played and DosBeep returns at once.
#[test]
fn test_audio() {
    if !kvm_available() || !Path::new("samples/audio_test/audio_test.exe").exists() { return; }
    let (out, code) = run_sample("samples/audio_test/audio_test.exe", 15);
    assert!(out.contains("Failed: 0"), "audio_test failed (exit {code}):\n{out}");
}

/// socket_test: SO32DLL loaded at run time, sockets over loopback, with
/// `select` on the OS/2 TCP/IP `fd_set` (a count and socket numbers).
#[test]
fn test_sockets_over_loopback() {
    if !kvm_available() || !Path::new("samples/socket_test/socket_test.exe").exists() { return; }
    let (out, _code) = run_sample("samples/socket_test/socket_test.exe", 20);
    for line in ["PASS  setsockopt(SO_REUSEADDR)", "PASS  getsockopt(SO_TYPE)",
                 "PASS  connect(loopback)", "PASS  accept()", "PASS  select(srv readable)",
                 "PASS  select(fd_count is the ready count)"] {
        assert!(out.contains(line), "missing '{line}':\n{out}");
    }
    let failures: Vec<&str> = out.lines().filter(|l| l.contains("FAIL")).collect();
    assert!(failures.is_empty(), "failures {failures:?}:\n{out}");
}

/// thread_test: DosCreateThread / DosWaitThread.
#[test]
fn test_thread() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/thread_test/thread_test.exe", 15);
    assert_eq!(code, 0, "thread_test exited with {code}\nstdout:\n{out}");
    assert!(out.contains("Hello from child thread!"), "thread output:\n{out}");
}

/// thread_exit_test: a thread ending with DosExit(EXIT_THREAD) or by
/// returning ends only itself; DosWaitThread reports it (once, by id or as
/// "any thread"; not thread 1 or the caller); 100 threads come and go; the
/// main thread carries on and the exit code is that of its final
/// DosExit(EXIT_PROCESS, 3).
#[test]
fn test_thread_exit_ends_only_the_thread() {
    const SAMPLE: &str = "samples/thread_exit_test/thread_exit_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let (out, code) = run_sample(SAMPLE, 30);
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with("thread_exit_test: ") && !l.contains(": done")).collect();
    assert!(checks.len() >= 21, "every check reported:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains(&format!("thread_exit_test: done {n}/{n}", n = checks.len())), "{out}");
    assert_eq!(code, 3, "exit code of DosExit(EXIT_PROCESS, 3)\n{out}");
}

/// thread_control_test: a thread spinning in a pure loop is stopped at once
/// by DosSuspendThread, a critical section and DosKillThread (its vCPU is
/// kicked out of the guest); suspend counts, CREATE_SUSPENDED, a killed
/// suspended thread ending on resume; DosSendSignalException reaches
/// thread 1's handler (also while it spins), and — with the program copied
/// to drive C as two child programs — a child process that asked for
/// signals handles XCPT_SIGNAL_INTR (exit code 7) while one that did not is
/// ended by it (exit code 1).
#[test]
fn test_thread_control_and_signal_exceptions() {
    const SAMPLE: &str = "samples/thread_control_test/thread_control_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let data_home = tempfile::tempdir().expect("tempdir");
    let drive_c = data_home.path().join("warpine/drive_c");
    std::fs::create_dir_all(&drive_c).unwrap();
    for name in ["THRDCF.EXE", "THRDCN.EXE"] {
        std::fs::copy(SAMPLE, drive_c.join(name)).unwrap();
    }
    let home = data_home.path().to_str().unwrap();
    let (out, code) = run_sample_env(SAMPLE, 90, &[("XDG_DATA_HOME", home)]);
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with("thread_control_test: ") && !l.contains(": done")).collect();
    assert!(checks.len() >= 51, "every check reported, the child ones too:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains(&format!("thread_control_test: done {n}/{n}", n = checks.len())), "{out}");
    assert_eq!(code, 0, "{out}");
}

const KILL_TEST: &str = "samples/kill_test/kill_test.exe";

/// Run kill_test as child `mode` (WARPINE_OS2ARGS = "child <mode>") with
/// `stdin` as its piped input; (stdout, exit code, drive C).
fn run_kill_test_mode(mode: &str, stdin: &[u8]) -> (String, i32, tempfile::TempDir) {
    use std::io::Write;
    let data_home = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(warpine())
        .arg(KILL_TEST)
        .env("WARPINE_HEADLESS", "1")
        .env("WARPINE_OS2ARGS", format!("child {mode}"))
        .env("XDG_DATA_HOME", data_home.path())
        .env("XDG_RUNTIME_DIR", data_home.path())
        .envs(PINNED_LOCALE.iter().copied())
        .env("RUST_LOG", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn warpine");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("kill_test {mode} did not end within 30s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1), data_home)
}

/// The file `name` on drive C under `data_home`, if any.
fn drive_c_file(data_home: &Path, name: &str) -> Option<String> {
    std::fs::read_dir(data_home.join("warpine/drive_c")).ok()?.flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .and_then(|e| std::fs::read_to_string(e.path()).ok())
}

/// kill_test: DosSleep / DosWaitEventSem interrupted by a signal with
/// ERROR_INTERRUPT, DosRead of a pipe made again after it; DosKillProcess
/// to child processes: the exit list runs with TC_KILLPROCESS and the
/// parent sees TC_KILLPROCESS, a handler that continues makes DosSleep fail
/// with ERROR_INTERRUPT, a child that ignores the signal is ended after the
/// grace period, DKP_PROCESSTREE reaches the grandchild.
#[test]
fn test_kill_process_and_interrupted_waits() {
    if !kvm_available() || !Path::new(KILL_TEST).exists() { return; }
    let (out, code) = run_sample(KILL_TEST, 90);
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with("kill_test: ") && !l.contains(": done")).collect();
    assert!(checks.len() >= 28, "every check reported:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains(&format!("kill_test: done {n}/{n}", n = checks.len())), "{out}");
    assert_eq!(code, 0, "{out}");
}

/// Ctrl+C in piped keyboard input is XCPT_SIGNAL_INTR for KbdCharIn, which
/// returns the next key.
#[test]
fn test_ctrl_c_from_the_keyboard_is_a_signal() {
    if !kvm_available() || !Path::new(KILL_TEST).exists() { return; }
    let (out, code, _c) = run_kill_test_mode("kbd", b"\x03x");
    assert!(!out.contains("FAIL") && out.matches(": ok").count() == 3, "{out}");
    assert_eq!(code, 0, "{out}");
}

/// The end of piped keyboard input ends the process like DosExit: its exit
/// list runs with TC_EXIT.
#[test]
fn test_exit_list_runs_at_the_end_of_piped_input() {
    if !kvm_available() || !Path::new(KILL_TEST).exists() { return; }
    let (out, code, c) = run_kill_test_mode("eof", b"");
    assert!(out.contains("kill_test: eof waiting") && !out.contains("FAIL"), "{out}");
    assert_eq!(drive_c_file(c.path(), "KT_EOF.OUT").as_deref(), Some("tc=0;"), "{out}");
    assert_eq!(code, 0, "{out}");
}

/// SIGTERM from the host is XCPT_SIGNAL_KILLPROC: the exit list runs with
/// TC_KILLPROCESS and the process ends with code 1.
#[test]
fn test_sigterm_runs_the_exit_list() {
    if !kvm_available() || !Path::new(KILL_TEST).exists() { return; }
    let data_home = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(warpine())
        .arg(KILL_TEST)
        .env("WARPINE_HEADLESS", "1")
        .env("WARPINE_OS2ARGS", "child exitlist")
        .env("XDG_DATA_HOME", data_home.path())
        .env("XDG_RUNTIME_DIR", data_home.path())
        .envs(PINNED_LOCALE.iter().copied())
        .env("RUST_LOG", "error")
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().expect("spawn warpine");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while drive_c_file(data_home.path(), "KT_EXITLIST.RDY").is_none() {
        if std::time::Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            panic!("the child never got ready");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let sent = Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    assert!(sent.success());
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() { break s; }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("SIGTERM did not end the process");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(1));
    assert_eq!(drive_c_file(data_home.path(), "KT_EXITLIST.OUT").as_deref(), Some("tc=3;"));
}

/// A named-pipe session directory left behind by a killed Warpine process
/// is removed when a new session starts; one whose process lives (here: a
/// record naming the test itself) is kept, and so is anything that is not
/// a pipe directory.
#[test]
fn test_stale_pipe_directories_are_removed() {
    if !kvm_available() || !Path::new(KILL_TEST).exists() { return; }
    use std::os::unix::fs::DirBuilderExt;
    let runtime = tempfile::tempdir().expect("tempdir");
    let uid = std::fs::metadata("/proc/self").map(|m| { use std::os::unix::fs::MetadataExt; m.uid() }).unwrap();
    let make = |pid: u32| {
        let p = runtime.path().join(format!("warpine-pipes-{uid}-{pid}"));
        std::fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        p
    };
    // A process that has ended (spawned and reaped: its pid is free).
    let mut gone = Command::new("true").spawn().unwrap();
    let gone_pid = gone.id();
    gone.wait().unwrap();
    let stale = make(gone_pid);
    std::fs::write(stale.join(format!("p-{gone_pid}")), format!("{gone_pid} 1\n")).unwrap();
    std::fs::write(stale.join("np-0123456789abcdef"), b"").unwrap();
    // A live session: the record names this test process and its start time.
    let me = std::process::id();
    let stat = std::fs::read_to_string(format!("/proc/{me}/stat")).unwrap();
    let start = stat[stat.rfind(')').unwrap() + 2..].split(' ').nth(19).unwrap().to_string();
    let live = make(me);
    std::fs::write(live.join(format!("p-{me}")), format!("{me} {start}\n")).unwrap();
    // Not Warpine's.
    let foreign = make(gone_pid + 1);
    std::fs::write(foreign.join("notes.txt"), b"keep").unwrap();

    // Any program: the sweep runs when a session starts ("eof" ends at once).
    let (out, code) = run_sample_env(KILL_TEST, 15,
                                     &[("XDG_RUNTIME_DIR", runtime.path().to_str().unwrap()),
                                       ("WARPINE_OS2ARGS", "child eof")]);
    assert_eq!(code, 0, "{out}");
    assert!(!stale.exists(), "the dead session's directory is removed");
    assert!(live.join(format!("p-{me}")).exists(), "a live session's directory is kept");
    assert!(foreign.join("notes.txt").exists(), "a directory with other files is kept");
}

/// run kill_test as "child win" in a VIO display (`vio`: None = the PM
/// window, or "fullscreen") under a GUI script that types Ctrl+C and then
/// closes the session; returns C:\KT_WIN.OUT.
fn run_kill_test_window(vio: Option<&str>) -> Option<String> {
    let has_script = Command::new("script").arg("--version").output().is_ok_and(|o| o.status.success());
    if !kvm_available() || !Path::new(KILL_TEST).exists() || !has_script {
        eprintln!("skipping: KVM, {KILL_TEST} or util-linux script not available");
        return None;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let session = dir.path().join("session.script");
    std::fs::write(&session, "2000 key ctrl+c\n3500 quit\n").unwrap();
    let mut cmd = Command::new("script");
    cmd.args(["-qec", &format!("timeout 30 {} {KILL_TEST}", warpine())])
        .arg("/dev/null")
        .envs(PINNED_LOCALE.iter().copied())
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("RUST_LOG", "error")
        .env("WARPINE_OS2ARGS", "child win")
        .env("XDG_DATA_HOME", dir.path())
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("WARPINE_GUI_SCRIPT", &session)
        .env_remove("WARPINE_VIO")
        .env_remove("WARPINE_HEADLESS")
        .stdout(Stdio::null());
    if let Some(vio) = vio {
        cmd.env("WARPINE_VIO", vio);
    }
    // Ended by XCPT_SIGNAL_KILLPROC: exit code 1 (signals.rs).
    let status = cmd.status().expect("run script");
    assert_eq!(status.code(), Some(1), "kill_test session: {status}");
    drive_c_file(dir.path(), "KT_WIN.OUT").or(Some(String::new()))
}

/// Ctrl+C typed in a VIO window is XCPT_SIGNAL_INTR; closing the window
/// asks the program to end: its exit list runs with TC_KILLPROCESS.
#[test]
fn gui_ctrl_c_and_close_in_a_vio_window() {
    let Some(out) = run_kill_test_window(None) else { return };
    assert_eq!(out, "intr;tc=3;");
}

/// The same in the full-screen text window.
#[test]
fn gui_ctrl_c_and_close_in_the_full_screen_text_window() {
    let Some(out) = run_kill_test_window(Some("fullscreen")) else { return };
    assert_eq!(out, "intr;tc=3;");
}

/// npipe_test: named pipes in one process and across processes, pipes
/// passed to child processes (an inherited handle, standard input,
/// standard output), the environment given to DosExecPgm, and
/// DosGiveSharedMem. The sample starts copies of itself.
#[test]
fn test_named_pipes_and_child_pipes() {
    const SAMPLE: &str = "samples/npipe_test/npipe_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let (out, code) = run_sample(SAMPLE, 60);
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with("npipe_test: ") && !l.contains(": done")).collect();
    assert!(checks.len() >= 60, "every check reported:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains(&format!("npipe_test: done {n}/{n}", n = checks.len())), "{out}");
    assert_eq!(code, 0, "{out}");
}

/// shmem_test: shared memory between a parent and a child (two Warpine
/// processes): named, gettable and giveable blocks at the same address in
/// both, a given block outliving the giver's DosFreeMem, a child's block
/// gone when it ends, one it gave kept by the parent (ADR-0049).
#[test]
fn test_shared_memory_between_processes() {
    const SAMPLE: &str = "samples/shmem_test/shmem_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let (out, code) = run_sample(SAMPLE, 60);
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with("shmem_test: ") && !l.contains(": done")).collect();
    assert!(checks.iter().filter(|l| l.contains(": child: ")).count() >= 13, "the child's checks:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains("shmem_test: done 29/29"), "{out}");
    assert_eq!(code, 0, "{out}");
}

/// shsem_test: shared semaphores between a parent and a child: named and
/// DC_SEM_SHARED events and mutexes opened by name and by handle, waits
/// woken from the other process, a muxwait list of shared events, a mutex
/// left owned by the ended child (ERROR_SEM_OWNER_DIED), a semaphore only
/// the child had open gone with it (ADR-0050).
#[test]
fn test_shared_semaphores_between_processes() {
    const SAMPLE: &str = "samples/shsem_test/shsem_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let (out, code) = run_sample(SAMPLE, 60);
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with("shsem_test: ") && !l.contains(": done")).collect();
    assert!(checks.iter().filter(|l| l.contains(": child: ")).count() >= 18, "the child's checks:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains("shsem_test: done 27/27"), "{out}");
    assert_eq!(code, 0, "{out}");
}

/// xcpt_thread_test: every thread has its own exception-handler chain (a
/// created thread starts with an empty one; exceptions reach only the
/// raising thread's handlers), and XCPT_CONTINUE_EXECUTION returns from
/// DosRaiseException with the caller's registers intact.
#[test]
fn test_exception_chains_are_per_thread() {
    const SAMPLE: &str = "samples/xcpt_thread_test/xcpt_thread_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let (out, code) = run_sample(SAMPLE, 30);
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with("xcpt_thread_test: ") && !l.contains(": done")).collect();
    assert!(checks.len() >= 17, "every check reported:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains(&format!("xcpt_thread_test: done {n}/{n}", n = checks.len())), "{out}");
    assert_eq!(code, 0, "{out}");
}

/// thread_last_test: thread 1 ends with DosExit(EXIT_THREAD) while another
/// thread runs; the process goes on until that last thread ends with
/// DosExit(EXIT_THREAD, 5), whose result becomes the exit code.
#[test]
fn test_thread_exit_of_the_last_thread_ends_the_process() {
    const SAMPLE: &str = "samples/thread_exit_test/thread_last_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let (out, code) = run_sample(SAMPLE, 15);
    assert!(out.contains("thread_exit_test: DosCreateThread(last thread): ok"), "{out}");
    assert!(out.contains("thread_exit_test: main thread ends"), "{out}");
    assert!(out.contains("thread_exit_test: last thread ends the process"), "{out}");
    assert!(!out.contains("FAIL"), "{out}");
    assert_eq!(code, 5, "exit code of the last thread's DosExit(EXIT_THREAD, 5)\n{out}");
}

/// Run one exitlist_test variant: every check before the process ends and
/// (when `runs` is set) in the exit-list routines passes; returns stdout.
fn run_exitlist_variant(exe: &str, code_want: i32, runs: bool) -> Option<String> {
    let sample = format!("samples/exitlist_test/{exe}");
    if !kvm_available() || !Path::new(&sample).exists() { return None; }
    let (out, code) = run_sample(&sample, 20);
    assert!(!out.contains("FAIL"), "{out}");
    assert!(out.lines().any(|l| l.starts_with("exitlist_test: done ")), "{out}");
    if runs {
        assert!(out.contains("exitlist_test: sequence BCAL"), "routines in order:\n{out}");
        assert!(out.contains("exitlist_test: exit list done "), "{out}");
        assert!(!out.contains("routine N"), "a removed routine ran:\n{out}");
        assert!(out.lines().filter(|l| l.contains(": routine ")).all(|l| l.ends_with("tc=0")), "TC_EXIT:\n{out}");
    } else {
        assert!(!out.contains(": routine "), "no exit list once thread 1 has ended:\n{out}");
    }
    assert_eq!(code, code_want, "{out}");
    Some(out)
}

/// exitlist_test: DosExitList error codes; DosExit(EXIT_PROCESS, 3) from
/// thread 1 runs the routines (by order, last added first, one added and
/// one removed while the list runs) before the process ends with code 3.
#[test]
fn test_exit_list_runs_at_dos_exit_process() {
    let Some(out) = run_exitlist_variant("exitlist_test.exe", 3, true) else { return };
    let routines = out.find("routine B").unwrap();
    assert!(out.find("exitlist_test: done ").unwrap() < routines, "the routines run at the end:\n{out}");
}

/// exitlist_thread: thread 2 ends the process (DosExit(EXIT_PROCESS, 4))
/// while thread 1 waits; the exit list runs.
#[test]
fn test_exit_list_runs_when_another_thread_ends_the_process() {
    run_exitlist_variant("exitlist_thread.exe", 4, true);
}

/// exitlist_mainlast: thread 1 is the last thread and ends with
/// DosExit(EXIT_THREAD, 6); the exit list runs.
#[test]
fn test_exit_list_runs_when_thread_1_ends_last() {
    run_exitlist_variant("exitlist_mainlast.exe", 6, true);
}

/// exitlist_nomain: thread 1 ends first; the last thread ending the
/// process does not run the exit list (thread 1 is gone).
#[test]
fn test_exit_list_does_not_run_after_thread_1_ended() {
    run_exitlist_variant("exitlist_nomain.exe", 5, false);
}

/// semquery_test: post counts (DosResetEventSem / DosQueryEventSem),
/// DosQueryMutexSem, DosOpenMuxWaitSem / DosAddMuxWaitSem /
/// DosDeleteMuxWaitSem / DosQueryMuxWaitSem and their error codes.
#[test]
fn test_semaphore_queries() {
    const SAMPLE: &str = "samples/semquery_test/semquery_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let (out, code) = run_sample(SAMPLE, 30);
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with("semquery_test: ") && !l.contains(": done")).collect();
    assert!(checks.len() >= 70, "every check reported:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains(&format!("semquery_test: done {n}/{n}", n = checks.len())), "{out}");
    assert_eq!(code, 0, "{out}");
}

/// semfollow_test: DCE_POSTONE / DCE_AUTORESET, ERROR_SEM_OWNER_DIED for a
/// mutex whose owner thread ended, ERROR_SEM_BUSY, muxwait wake-ups, pipe
/// semaphores (DosSetNPipeSem / DosQueryNPipeSemState), and a suballocation
/// pool freed with DosFreeMem without DosSubUnsetMem.
#[test]
fn test_semaphore_pipe_and_suballoc_follow_ups() {
    const SAMPLE: &str = "samples/semfollow_test/semfollow_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let (out, code) = run_sample(SAMPLE, 60);
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with("semfollow_test: ") && !l.contains(": done")).collect();
    assert!(checks.len() >= 60, "every check reported:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains(&format!("semfollow_test: done {n}/{n}", n = checks.len())), "{out}");
    assert_eq!(code, 0, "{out}");
}

/// kernel2_test: DosEnterCritSec / DosExitCritSec, DosAsyncTimer /
/// DosStartTimer / DosStopTimer, DosSearchPath, DosQueryModuleName and the
/// DosSub* suballocator. Drive C is a temporary directory (the sample
/// creates and removes `K2DIR` in its current directory).
#[test]
fn test_kernel_batch2() {
    const SAMPLE: &str = "samples/kernel2_test/kernel2_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let dir = tempfile::tempdir().expect("tempdir");
    let xdg = dir.path().to_string_lossy().into_owned();
    let (out, code) = run_sample_env(SAMPLE, 60, &[("XDG_DATA_HOME", xdg.as_str())]);
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with("kernel2_test: ") && !l.contains(": done") && !l.contains(": note "))
        .collect();
    assert!(checks.len() >= 90, "every check reported:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains(&format!("kernel2_test: done {n}/{n}", n = checks.len())), "{out}");
    assert!(out.contains("kernel2_test: note exe = C:\\KERNEL2_TEST.EXE"), "{out}");
    assert_eq!(code, 0, "{out}");
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

/// ne_hello: 16-bit NE format DosWrite + DosExit — pure assembly, no CRT.
#[test]
fn test_ne_hello() {
    if !kvm_available() { return; }
    let (out, code) = run_sample("samples/ne_hello/ne_hello.exe", 15);
    assert_eq!(code, 0, "ne_hello exited with {code}\nstdout:\n{out}");
    assert!(
        out.contains("Hello from NE (16-bit OS/2)!"),
        "unexpected output:\n{out}"
    );
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

/// rust_hello: Rust guest binary built with the warpine-os2 crate family.
///
/// Skipped gracefully when:
/// - KVM is unavailable
/// - `lx-link` has not been built yet (`target/debug/lx-link` absent)
/// - The nightly build of `rust_hello` fails (toolchain or dependency issue)
#[test]
fn test_rust_hello() {
    if !kvm_available() { return; }

    let workspace_root = env!("CARGO_MANIFEST_DIR");

    // lx-link must be pre-built; skip rather than fail if it is missing.
    let lx_link = format!("{workspace_root}/target/debug/lx-link");
    if !Path::new(&lx_link).exists() {
        return;
    }

    // Build rust_hello with the nightly Rust guest toolchain.
    let target_json = format!("{workspace_root}/targets/i686-warpine-os2.json");
    let rust_hello_dir = format!("{workspace_root}/samples/rust_hello");

    // Prepend the debug directory to PATH so lx-link is found at link time.
    let debug_dir = format!("{workspace_root}/target/debug");
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{debug_dir}:{existing_path}");

    let status = Command::new("cargo")
        .args([
            "+nightly",
            "build",
            "-Z", "build-std=core,alloc",
            "-Z", "build-std-features=compiler-builtins-mem",
            "-Z", "json-target-spec",
            "--target", &target_json,
        ])
        .current_dir(&rust_hello_dir)
        .env("PATH", &new_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // If nightly is unavailable or the build fails for any reason, skip.
    let ok = match status {
        Ok(s) => s.success(),
        Err(_) => false,
    };
    if !ok {
        return;
    }

    let exe = "samples/rust_hello/target/i686-warpine-os2/debug/rust_hello.exe";
    let (out, code) = run_sample(exe, 15);
    assert_eq!(code, 0, "rust_hello exited with {code}\nstdout:\n{out}");
    assert!(
        out.contains("Hello from Rust on Warpine!"),
        "unexpected output:\n{out}"
    );
}

// ── GUI regression tests ─────────────────────────────────────────────────────
//
// These drive a PM sample with a scripted session (`WARPINE_GUI_SCRIPT`, see
// `src/gui/gui_script.rs`) on SDL's dummy video driver, so no display is
// needed, and compare the dumped desktop against a stored reference.
//
// The reference is not an image but a *tile signature*: the average colour of
// every 16×16-pixel tile. It is a few kilobytes of text, survives 1-pixel
// anti-aliasing differences, and a mismatch names the tiles that changed, so
// a failure says where on the desktop to look.
//
// After an intentional rendering change, regenerate the references with
//   WARPINE_UPDATE_GOLDEN=1 cargo test --test integration gui_
// and review the diff of `tests/golden/*.sig`.

const TILE: usize = 16;
/// Largest per-channel difference of a tile average that still counts as equal.
const TILE_TOLERANCE: i32 = 6;

/// Run `sample` under a GUI script that captures the desktop `count` times,
/// with extra environment variables `env` for warpine, and return the
/// captures in order as `(width, height, rgb bytes)`. `steps` are script
/// lines; `{dump0}`, `{dump1}`, … name the capture paths (`{dump}` is
/// `{dump0}`).
fn run_gui_sample_dumps_env(sample: &str, steps: &str, count: usize, env: &[(&str, &str)])
                            -> Vec<(usize, usize, Vec<u8>)> {
    let dir = tempfile::tempdir().expect("tempdir");
    let dumps: Vec<_> = (0..count).map(|i| dir.path().join(format!("desktop{i}.ppm"))).collect();
    let script = dir.path().join("session.script");
    let mut text = steps.replace("{dump}", "{dump0}");
    for (i, dump) in dumps.iter().enumerate() {
        text = text.replace(&format!("{{dump{i}}}"), dump.to_str().unwrap());
    }
    std::fs::write(&script, text).unwrap();
    run_gui_script_env(sample, &script, false, env);

    dumps.iter().map(|dump| {
        let ppm = std::fs::read(dump)
            .unwrap_or_else(|e| panic!("{sample} wrote no desktop dump {}: {e}", dump.display()));
        parse_ppm(&ppm)
    }).collect()
}

/// Run `sample` on SDL's dummy driver under the GUI script file `script` and
/// wait for it to exit (30 s at most). Returns its stdout when `capture` is
/// set, an empty string otherwise.
fn run_gui_script(sample: &str, script: &Path, capture: bool) -> String {
    run_gui_script_env(sample, script, capture, &[])
}

/// [`run_gui_script`] with extra environment variables for warpine.
fn run_gui_script_env(sample: &str, script: &Path, capture: bool, env: &[(&str, &str)]) -> String {
    // A fresh drive C: per run, never the user's own (`env` may name another).
    let data_home = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(warpine())
        .arg(sample)
        .env_remove("WARPINE_COMPOSITOR")
        .env_remove("WARPINE_DESKTOP")
        .env("XDG_DATA_HOME", data_home.path())
        // Named-pipe and thread-session directories go there too, never
        // into the user's own runtime directory.
        .env("XDG_RUNTIME_DIR", data_home.path())
        .envs(env.iter().copied())
        .envs(PINNED_LOCALE.iter().copied())
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("WARPINE_GUI_SCRIPT", script)
        .env_remove("WARPINE_HEADLESS")
        .env_remove("WARPINE_DESKTOP_W")
        .env_remove("WARPINE_DESKTOP_H")
        .env("RUST_LOG", "error")
        .stdout(if capture { Stdio::piped() } else { Stdio::null() })
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn warpine for {sample}: {e}"));
    // Read stdout on a thread so a chatty sample cannot fill the pipe and block.
    let reader = child.stdout.take().map(|out| std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        std::io::BufReader::new(out).read_to_string(&mut buf).ok();
        buf
    }));
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while child.try_wait().expect("try_wait failed").is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            panic!("{sample} did not finish its GUI script within 30s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    reader.map(|r| r.join().unwrap()).unwrap_or_default()
}

/// Colour of desktop pixel `(x, y)` (origin top-left) as `0xRRGGBB`.
fn gui_pixel(desktop: &(usize, usize, Vec<u8>), x: usize, y: usize) -> u32 {
    let (w, h, rgb) = desktop;
    assert!(x < *w && y < *h, "pixel ({x}, {y}) is outside the {w}x{h} desktop");
    let i = (y * w + x) * 3;
    (rgb[i] as u32) << 16 | (rgb[i + 1] as u32) << 8 | rgb[i + 2] as u32
}

/// Decode a binary PPM (P6, maxval 255) into `(width, height, rgb bytes)`.
fn parse_ppm(data: &[u8]) -> (usize, usize, Vec<u8>) {
    // Header: "P6\n<w> <h>\n255\n" — three newline-terminated lines.
    let mut pos = 0;
    let mut line = || {
        let end = pos + data[pos..].iter().position(|&b| b == b'\n').expect("truncated PPM header");
        let text = std::str::from_utf8(&data[pos..end]).expect("PPM header is not ASCII");
        pos = end + 1;
        text
    };
    assert_eq!(line(), "P6", "not a binary PPM");
    let dims: Vec<usize> = line().split_whitespace().map(|n| n.parse().unwrap()).collect();
    assert_eq!(line(), "255");
    let (w, h) = (dims[0], dims[1]);
    assert_eq!(data.len() - pos, w * h * 3, "PPM pixel data has the wrong length");
    (w, h, data[pos..].to_vec())
}

/// Average colour of each `TILE`×`TILE` tile, row by row.
fn tile_signature(w: usize, h: usize, rgb: &[u8]) -> Vec<Vec<[u8; 3]>> {
    (0..h.div_ceil(TILE)).map(|ty| {
        (0..w.div_ceil(TILE)).map(|tx| {
            let (mut sum, mut n) = ([0u32; 3], 0u32);
            for y in ty * TILE..((ty + 1) * TILE).min(h) {
                for x in tx * TILE..((tx + 1) * TILE).min(w) {
                    for c in 0..3 { sum[c] += rgb[(y * w + x) * 3 + c] as u32; }
                    n += 1;
                }
            }
            [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
        }).collect()
    }).collect()
}

fn signature_to_text(w: usize, h: usize, sig: &[Vec<[u8; 3]>]) -> String {
    let mut out = format!("{w} {h} {TILE}\n");
    for row in sig {
        for t in row { out.push_str(&format!("{:02x}{:02x}{:02x}", t[0], t[1], t[2])); }
        out.push('\n');
    }
    out
}

fn signature_from_text(text: &str) -> (usize, usize, Vec<Vec<[u8; 3]>>) {
    let mut lines = text.lines();
    let head: Vec<usize> = lines.next().unwrap_or("").split_whitespace()
        .map(|n| n.parse().expect("bad golden header")).collect();
    assert_eq!(head.get(2), Some(&TILE), "golden file uses a different tile size");
    let rows = lines.map(|l| {
        l.as_bytes().chunks(6).map(|c| {
            let v = u32::from_str_radix(std::str::from_utf8(c).unwrap(), 16).expect("bad golden tile");
            [(v >> 16) as u8, (v >> 8) as u8, v as u8]
        }).collect()
    }).collect();
    (head[0], head[1], rows)
}

/// Tiles whose averages differ by more than the tolerance, as `(tile x, tile y)`.
fn differing_tiles(a: &[Vec<[u8; 3]>], b: &[Vec<[u8; 3]>]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (ty, (ra, rb)) in a.iter().zip(b).enumerate() {
        for (tx, (ta, tb)) in ra.iter().zip(rb).enumerate() {
            if (0..3).any(|c| (ta[c] as i32 - tb[c] as i32).abs() > TILE_TOLERANCE) {
                out.push((tx, ty));
            }
        }
    }
    out
}

/// Run `sample` under `steps` and compare its desktop with `tests/golden/<name>.sig`.
/// Skipped when KVM or the (git-ignored, Open Watcom-built) sample is missing.
fn check_gui_golden(name: &str, sample: &str, steps: &str) {
    check_gui_golden_env(name, sample, steps, &[]);
}

/// [`check_gui_golden`] with extra environment variables for warpine. The
/// golden files are written by runs without them.
fn check_gui_golden_env(name: &str, sample: &str, steps: &str, env: &[(&str, &str)]) {
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping {name}: KVM or {sample} not available");
        return;
    }
    let (w, h, rgb) = run_gui_sample_dumps_env(sample, steps, 1, env).remove(0);
    let sig = tile_signature(w, h, &rgb);
    let golden_path = Path::new("tests/golden").join(format!("{name}.sig"));

    if std::env::var_os("WARPINE_UPDATE_GOLDEN").is_some() && env.is_empty() {
        std::fs::create_dir_all("tests/golden").unwrap();
        std::fs::write(&golden_path, signature_to_text(w, h, &sig)).unwrap();
        eprintln!("updated {}", golden_path.display());
        return;
    }
    let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|_| panic!(
        "missing {}; create it with WARPINE_UPDATE_GOLDEN=1 cargo test --test integration gui_",
        golden_path.display()));
    let (gw, gh, gsig) = signature_from_text(&golden);
    assert_eq!((w, h), (gw, gh), "{name}: desktop size changed");
    let diff = differing_tiles(&sig, &gsig);
    assert!(diff.is_empty(),
        "{name}: {} of {} tiles differ from {} (first, as tile x,y of {TILE}px tiles: {:?}). \
         If the change is intended, regenerate with WARPINE_UPDATE_GOLDEN=1.",
        diff.len(), sig.len() * sig[0].len(), golden_path.display(),
        &diff[..diff.len().min(12)]);
}

/// Run 4OS2 on a pseudo-terminal (VIO programs get a display only when
/// stdout is a terminal; `script` from util-linux provides one) with
/// `WARPINE_VIO` set to `vio` (None: the default), type `ver` and Enter, and
/// A scripted VIO session that ends with `quit` closes the program's window
/// or screen: the program gets `XCPT_SIGNAL_KILLPROC` and runs its exit list
/// (ADR-0034), and its own handler chooses the exit code (4OS2 exits with 3).
/// So any ordinary exit is a closed session; being killed by a signal or by
/// the `timeout` wrapper (124) is a failure. The callers check the dumps and
/// files the session produced, which catches a crash before `quit`.
fn assert_session_closed(status: std::process::ExitStatus, what: &str) {
    const TIMEOUT_EXIT: i32 = 124;
    match status.code() {
        Some(code) if code != TIMEOUT_EXIT => {}
        _ => panic!("{what} session did not end by its quit step: {status}"),
    }
}

/// return the dumped screen. `None` when KVM, 4OS2 or `script` is missing.
fn run_4os2_session(vio: Option<&str>) -> Option<(usize, usize, Vec<u8>)> {
    run_4os2_script(vio, &["screen"], |dumps| {
        format!("1500 text ver\n1700 key enter\n3000 dump {}\n3100 quit\n", dumps[0].display())
    }).map(|mut frames| frames.remove(0))
}

/// Run 4OS2 under a pseudo-terminal with the GUI script `script` makes from
/// the paths of the `dumps` it writes; returns each dump decoded, in order.
/// `None` (test skipped) without KVM, the gitignored 4OS2 binary or
/// util-linux `script`.
fn run_4os2_script(vio: Option<&str>, dumps: &[&str], script: impl FnOnce(&[std::path::PathBuf]) -> String)
                   -> Option<Vec<(usize, usize, Vec<u8>)>> {
    let sample = "samples/4os2/4os2.exe";
    let has_script = Command::new("script").arg("--version").output().is_ok_and(|o| o.status.success());
    if !kvm_available() || !Path::new(sample).exists() || !has_script {
        eprintln!("skipping: KVM, {sample} or util-linux script not available");
        return None;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let paths: Vec<_> = dumps.iter().map(|name| dir.path().join(format!("{name}.ppm"))).collect();
    let session = dir.path().join("session.script");
    std::fs::write(&session, script(&paths)).unwrap();
    let mut cmd = Command::new("script");
    cmd.args(["-qec", &format!("timeout 30 {} {sample}", warpine())])
        .arg("/dev/null")
        .envs(PINNED_LOCALE.iter().copied())
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("RUST_LOG", "error")
        .env("WARPINE_GUI_SCRIPT", &session)
        // Drive C: in the session's temporary directory, not the user's own.
        .env("XDG_DATA_HOME", dir.path())
        // Named-pipe and thread-session directories go there too, never
        // into the user's own runtime directory.
        .env("XDG_RUNTIME_DIR", dir.path())
        .env_remove("WARPINE_VIO")
        .stdout(Stdio::null());
    if let Some(vio) = vio {
        cmd.env("WARPINE_VIO", vio);
    }
    let status = cmd.status().expect("run script");
    assert_session_closed(status, "4OS2");
    Some(paths.iter().map(|path| {
        let ppm = std::fs::read(path).unwrap_or_else(|_| panic!("no dump {} was written", path.display()));
        parse_ppm(&ppm)
    }).collect())
}

/// Pels of CGA light grey, the colour of 4OS2's text.
fn light_grey_pels(rgb: &[u8]) -> usize {
    const LIGHT_GREY: [u8; 3] = [0xAA, 0xAA, 0xAA];
    rgb.chunks(3).filter(|p| *p == LIGHT_GREY).count()
}

/// Run samples/mou_test under a pseudo-terminal (a VIO program gets a
/// display only when stdout is a terminal), windowed or with `WARPINE_VIO`
/// = `vio`, clicking, right-clicking and dragging on the cells the sample
/// expects. `origin` is the desktop point of the top-left corner of the
/// cells. Returns the sample's result file (C:\MOU_TEST.OUT), or `None`
/// when the test cannot run here.
fn run_mou_session(vio: Option<&str>, origin: (i32, i32)) -> Option<String> {
    let sample = "samples/mou_test/mou_test.exe";
    let has_script = Command::new("script").arg("--version").output().is_ok_and(|o| o.status.success());
    if !kvm_available() || !Path::new(sample).exists() || !has_script {
        eprintln!("skipping: KVM, {sample} or util-linux script not available");
        return None;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    // The centre of cell (row, col): cells are 8 × 16 pels.
    let at = |row: i32, col: i32| (origin.0 + col * 8 + 4, origin.1 + row * 16 + 8);
    let ((x1, y1), (x2, y2), (x3, y3), (x4, y4)) = (at(5, 10), at(7, 20), at(9, 30), at(9, 40));
    let session = dir.path().join("session.script");
    std::fs::write(&session, format!(
        "2000 click {x1} {y1}\n2400 rclick {x2} {y2}\n2800 drag {x3} {y3} {x4} {y4}\n4500 quit\n")).unwrap();
    let mut cmd = Command::new("script");
    cmd.args(["-qec", &format!("timeout 30 {} {sample}", warpine())])
        .arg("/dev/null")
        .envs(PINNED_LOCALE.iter().copied())
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("RUST_LOG", "error")
        // C: is $XDG_DATA_HOME/warpine/drive_c: the sample's file lands here.
        .env("XDG_DATA_HOME", dir.path())
        // Named-pipe and thread-session directories go there too, never
        // into the user's own runtime directory.
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("WARPINE_GUI_SCRIPT", &session)
        .env_remove("WARPINE_VIO")
        .stdout(Stdio::null());
    if let Some(vio) = vio {
        cmd.env("WARPINE_VIO", vio);
    }
    let status = cmd.status().expect("run script");
    assert_session_closed(status, "mou_test");
    let drive_c = dir.path().join("warpine/drive_c");
    let file = std::fs::read_dir(&drive_c).expect("no C: drive").flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("MOU_TEST.OUT"))
        .expect("mou_test wrote no result file");
    Some(std::fs::read_to_string(file.path()).unwrap())
}

/// Every check of the sample passed and it saw all the scripted events.
fn assert_mou_results(out: &str) {
    let failures: Vec<&str> = out.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failures.is_empty(), "failures: {failures:?}\n{out}");
    assert!(out.contains("ok drag: release at (9, 40)"), "events missing:\n{out}");
    assert!(out.contains("done: 0 failed"), "incomplete:\n{out}");
}

/// MOUCALLS in a windowed VIO session: the pointer over the frame's cells
/// reaches MouReadEventQue in text cells. The frame is the first cascaded
/// one (32, 32 from the desktop's top-left: FRAME_CASCADE_ORIGIN); its
/// cells start inside the 2-pel border, below the 20-pel title bar.
#[test]
fn mouse_events_reach_a_windowed_vio_program() {
    let Some(out) = run_mou_session(None, (32 + 2, 32 + 20)) else { return };
    assert_mou_results(&out);
}

/// MOUCALLS full screen: the 640 × 400 text window maps the pointer to cells.
#[test]
fn mouse_events_reach_a_full_screen_vio_program() {
    let Some(out) = run_mou_session(Some("fullscreen"), (0, 0)) else { return };
    assert_mou_results(&out);
}

/// WARPINE_VIO=fullscreen: the separate text window replays the GUI script —
/// `ver` typed and Enter — and `dump` writes its 640×400 frame.
#[test]
fn text_mode_window_replays_a_gui_script_and_dumps_the_screen() {
    let Some((w, h, rgb)) = run_4os2_session(Some("fullscreen")) else { return };
    assert_eq!((w, h), (640, 400), "80×25 cells of 8×16 pels");
    let lit = light_grey_pels(&rgb);
    assert!(lit > 1000, "only {lit} light-grey pels: no text on the screen");
}

/// By default a window-compatible VIO program runs in a window on the PM
/// desktop: the dump is the whole desktop, with the active frame's navy
/// title bar and 4OS2's text in it.
#[test]
fn vio_program_runs_in_a_pm_window_by_default() {
    let Some((w, h, rgb)) = run_4os2_session(None) else { return };
    assert!((w, h) != (640, 400), "the PM desktop, not the text window");
    let lit = light_grey_pels(&rgb);
    assert!(lit > 1000, "only {lit} light-grey pels: no text in the window");
    const NAVY: [u8; 3] = [0x00, 0x00, 0x80];
    let title = rgb.chunks(3).filter(|p| *p == NAVY).count();
    assert!(title > 1000, "only {title} navy pels: no active title bar");
}

/// Alt+Home switches a running VIO program from its PM window to full
/// screen (the dump is the 640×400 text screen) and back to the desktop;
/// the program and its screen carry on across both switches, and keys typed
/// full screen reach it.
#[test]
fn alt_home_switches_a_vio_program_between_window_and_full_screen() {
    let Some(frames) = run_4os2_script(None, &["window", "full", "back"], |d| format!(
        "1500 text ver\n1700 key enter\n2800 dump {}\n3000 key alt+home\n\
         4000 text ver\n4200 key enter\n5000 dump {}\n5500 key alt+home\n6500 dump {}\n6600 quit\n",
        d[0].display(), d[1].display(), d[2].display())) else { return };
    let [(ww, wh, window), (fw, fh, full), (bw, bh, back)] = &frames[..] else { unreachable!() };
    assert!((*ww, *wh) != (640, 400), "starts in a window on the desktop");
    assert_eq!((*fw, *fh), (640, 400), "full screen: the 80×25 text screen");
    assert_eq!((*bw, *bh), (*ww, *wh), "back on the desktop");
    let (before, full_lit, after) = (light_grey_pels(window), light_grey_pels(full), light_grey_pels(back));
    assert!(before > 1000, "only {before} light-grey pels in the window");
    // `ver` typed full screen adds its output to what was there.
    assert!(full_lit > before, "full screen shows the screen and the new output: {full_lit} <= {before}");
    assert!(after >= full_lit, "the window shows the same screen again: {after} < {full_lit}");
    const NAVY: [u8; 3] = [0x00, 0x00, 0x80];
    let title = back.chunks(3).filter(|p| *p == NAVY).count();
    assert!(title > 1000, "only {title} navy pels: the VIO window is not active again");
}

/// WARPINE_VIO=fullscreen starts full screen; Alt+Home then opens the PM
/// desktop with the program's screen in a window, and switches back.
#[test]
fn alt_home_from_full_screen_opens_the_pm_window() {
    let Some(frames) = run_4os2_script(Some("fullscreen"), &["full", "window", "again"], |d| format!(
        "1500 text ver\n1700 key enter\n2800 dump {}\n3000 key alt+home\n4000 dump {}\n\
         4200 key alt+home\n5200 dump {}\n5300 quit\n",
        d[0].display(), d[1].display(), d[2].display())) else { return };
    let sizes: Vec<_> = frames.iter().map(|(w, h, _)| (*w, *h)).collect();
    assert_eq!(sizes[0], (640, 400), "starts full screen");
    assert!(sizes[1] != (640, 400), "then the desktop");
    assert_eq!(sizes[2], (640, 400), "full screen again");
    let lit: Vec<_> = frames.iter().map(|(_, _, rgb)| light_grey_pels(rgb)).collect();
    assert!(lit.iter().all(|&n| n > 1000), "the screen is shown every time: {lit:?}");
}

/// A desktop dump: width, height and RGB pels.
type Screen = (usize, usize, Vec<u8>);
/// What [`run_4os2_windowed`] returns: the dumps, drive C:'s host directory
/// and the temporary directory holding it.
type WindowedSession = (Vec<Screen>, std::path::PathBuf, tempfile::TempDir);

/// Run 4OS2 in a window on the PM desktop, on a pseudo-terminal like
/// [`run_4os2_session`], under the GUI script `steps` (`{dumpN}` stands for
/// the N-th of `dumps` screen dumps), with drive C: in a fresh directory.
/// Returns the dumps and C:'s host directory (removed when the `TempDir`
/// is dropped); `None` when KVM, 4OS2 or `script` is missing.
fn run_4os2_windowed(steps: &str, dumps: usize) -> Option<WindowedSession> {
    run_4os2_windowed_with(steps, dumps, &[])
}

/// [`run_4os2_windowed`] with `files` (host paths) copied to drive C:'s root
/// first; `None` also when one of them is missing.
fn run_4os2_windowed_with(steps: &str, dumps: usize, files: &[&str]) -> Option<WindowedSession> {
    let sample = "samples/4os2/4os2.exe";
    let has_script = Command::new("script").arg("--version").output().is_ok_and(|o| o.status.success());
    if !kvm_available() || !Path::new(sample).exists() || !has_script
        || files.iter().any(|f| !Path::new(f).exists()) {
        eprintln!("skipping: KVM, {sample}, {files:?} or util-linux script not available");
        return None;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let drive_c = dir.path().join("warpine/drive_c");
    std::fs::create_dir_all(&drive_c).unwrap();
    for f in files {
        let name = Path::new(f).file_name().unwrap();
        std::fs::copy(f, drive_c.join(name)).unwrap();
    }
    let paths: Vec<_> = (0..dumps).map(|i| dir.path().join(format!("screen{i}.ppm"))).collect();
    let mut text = steps.to_string();
    for (i, p) in paths.iter().enumerate() {
        text = text.replace(&format!("{{dump{i}}}"), p.to_str().unwrap());
    }
    let session = dir.path().join("session.script");
    std::fs::write(&session, text).unwrap();
    let status = Command::new("script")
        .args(["-qec", &format!("timeout 40 {} {sample}", warpine())])
        .arg("/dev/null")
        .envs(PINNED_LOCALE.iter().copied())
        .env("SDL_VIDEODRIVER", "dummy")
        .env("SDL_AUDIODRIVER", "dummy")
        .env("RUST_LOG", "error")
        .env("WARPINE_GUI_SCRIPT", &session)
        // Drive C: is $XDG_DATA_HOME/warpine/drive_c.
        .env("XDG_DATA_HOME", dir.path())
        // Named-pipe and thread-session directories go there too, never
        // into the user's own runtime directory.
        .env("XDG_RUNTIME_DIR", dir.path())
        .env_remove("WARPINE_VIO")
        .env_remove("WARPINE_COMPOSITOR")
        .env_remove("WARPINE_DESKTOP")
        .env_remove("WARPINE_DESKTOP_W")
        .env_remove("WARPINE_DESKTOP_H")
        .stdout(Stdio::null())
        .status()
        .expect("run script");
    assert_session_closed(status, "4OS2");
    let shots = paths.iter().map(|p| {
        parse_ppm(&std::fs::read(p).unwrap_or_else(|e| panic!("no screen dump {}: {e}", p.display())))
    }).collect();
    // `drive_c` was created above.
    Some((shots, drive_c, dir))
}

/// Whether drive C: (host directory `drive_c`) holds a directory named
/// `name`, in any case.
fn has_dir(drive_c: &Path, name: &str) -> bool {
    std::fs::read_dir(drive_c).map(|d| d.flatten().any(|e| {
        e.file_name().to_string_lossy().eq_ignore_ascii_case(name) && e.path().is_dir()
    })).unwrap_or(false)
}

/// Pels of the active title bar's navy in desktop row `y`.
fn navy_in_row(shot: &(usize, usize, Vec<u8>), y: usize) -> usize {
    (0..shot.0).filter(|&x| gui_pixel(shot, x, y) == 0x00_0080).count()
}

/// Geometry of the VIO window on the default 1024 × 768 desktop: the frame
/// of an 80 × 25 screen (644 × 422 with its 2-pel border and 20-pel title
/// bar) at the first cascade position, 32 pels from the top-left corner.
const VIO_FRAME_X: usize = 32;
const VIO_FRAME_TOP: usize = 32;
const VIO_FRAME_CX: usize = 644;
const VIO_FRAME_CY: usize = 422;
/// The top-left of the text cells, and a cell's size.
const VIO_CELLS_X: usize = VIO_FRAME_X + 2;
const VIO_CELLS_Y: usize = VIO_FRAME_TOP + 20;
/// A row of the title bar, clear of its buttons' pictograms.
const VIO_TITLE_Y: usize = VIO_FRAME_TOP + 3;

/// The VIO window has minimize and maximize buttons and a sizing border:
/// dragging its bottom-right corner makes it smaller (scroll bars appear),
/// dragging it past the screen's size stops at the whole screen, the
/// maximize button fits it to the screen, Alt+F5 restores and Alt+F9
/// minimizes it to a title bar along the bottom of the desktop.
#[test]
fn vio_window_resizes_maximizes_restores_and_minimizes() {
    let corner = (VIO_FRAME_X + VIO_FRAME_CX - 2, VIO_FRAME_TOP + VIO_FRAME_CY - 2);
    // Shrunk by 275 × 150 pels: the maximize button of the smaller frame
    // (close, then maximize, 16 pels each with 2-pel gaps).
    let small_right = VIO_FRAME_X + VIO_FRAME_CX - 275;
    let max_button = small_right - 2 - 16 - 2 - 8;
    let steps = format!("\
        1500 text ver\n1600 key enter\n\
        2000 drag {cx} {cy} {sx} {sy}\n\
        2300 dump {{dump0}}\n\
        2400 drag {sx} {sy} 1000 760\n\
        2700 dump {{dump1}}\n\
        2800 drag {cx} {cy} {sx} {sy}\n\
        3100 click {max_button} {ty}\n\
        3400 dump {{dump2}}\n\
        3500 key alt+f5\n\
        3800 dump {{dump3}}\n\
        3900 key alt+f9\n\
        4200 dump {{dump4}}\n\
        4300 quit\n",
        cx = corner.0, cy = corner.1, sx = corner.0 - 275, sy = corner.1 - 150, ty = VIO_FRAME_TOP + 10);
    let Some((shots, _, _dir)) = run_4os2_windowed(&steps, 5) else { return };
    let full = navy_in_row(&shots[1], VIO_TITLE_Y);
    let small = navy_in_row(&shots[0], VIO_TITLE_Y);
    assert!(small + 200 < full, "shrunk: {small} navy title pels, {full} at full size");
    // Scroll bars: the grey track and the light-grey buttons inside the frame.
    let track = |s: &(usize, usize, Vec<u8>)| (VIO_FRAME_X..small_right).flat_map(|x| {
        (VIO_CELLS_Y..VIO_FRAME_TOP + VIO_FRAME_CY - 150).map(move |y| (x, y))
    }).filter(|&(x, y)| gui_pixel(s, x, y) == 0xA0_A0A0).count();
    assert!(track(&shots[0]) > 300, "scroll bars in the smaller window: {}", track(&shots[0]));
    // Dragged past the screen: the whole screen, no larger.
    assert_eq!(gui_pixel(&shots[1], VIO_FRAME_X + VIO_FRAME_CX + 4, VIO_TITLE_Y), 0x40_8040,
               "the desktop right of the full-size frame");
    assert_eq!(track(&shots[1]), 0, "no scroll bars at full size");
    // Maximized: the whole screen again; restored: the smaller window.
    let maxed = navy_in_row(&shots[2], VIO_TITLE_Y);
    assert!(maxed + 50 > full, "maximized: {maxed} navy pels, full size {full}");
    assert_eq!(navy_in_row(&shots[3], VIO_TITLE_Y), small, "restored to the smaller size");
    // Minimized: no title bar at the top any more, one along the bottom.
    assert_eq!(navy_in_row(&shots[4], VIO_TITLE_Y), 0, "minimized");
    let bottom = (shots[4].1 - 40..shots[4].1).map(|y| navy_in_row(&shots[4], y)).sum::<usize>();
    assert!(bottom > 500, "a minimized title bar at the bottom: {bottom} navy pels");
}

/// The system menu's Copy All copies the screen and Paste types it back:
/// with the prompt `md ` and a screen of `rem` lines, the pasted screen
/// ends with the command line `md Mq9`, which creates the directory.
#[test]
fn vio_window_copy_all_and_paste_round_trip() {
    let steps = "\
        1500 text prompt md$s\n1600 key enter\n\
        1900 text for /l %i in (1,1,40) echo rem\n2000 key enter\n\
        2600 text Mq9\n\
        2800 key alt+space\n2900 key y\n\
        3000 key esc\n\
        3100 dump {dump0}\n\
        3200 key alt+space\n3300 key p\n\
        5000 key enter\n\
        5600 quit\n";
    let Some((_, drive_c, _dir)) = run_4os2_windowed(steps, 1) else { return };
    assert!(has_dir(&drive_c, "Mq9"), "the pasted `md Mq9` ran: {:?}",
            std::fs::read_dir(&drive_c).map(|d| d.flatten().map(|e| e.file_name()).collect::<Vec<_>>()));
}

/// Pels of row `row` of the VIO window's cells that are not black, from the
/// second cell on (the first pels of the first cell are the frame's edge).
fn lit_in_cell_row(shot: &(usize, usize, Vec<u8>), row: usize) -> usize {
    (VIO_CELLS_Y + row * 16..VIO_CELLS_Y + row * 16 + 16).flat_map(|y| {
        (VIO_CELLS_X + 8..VIO_CELLS_X + 80 * 8).map(move |x| (x, y))
    }).filter(|&(x, y)| gui_pixel(shot, x, y) != 0).count()
}

/// 4OS2's `cls` writes `ESC[2J`, which the VIO screen carries out (it was
/// shown as a literal `[2J`); `echo … > file`, `>>` and an external program's
/// `> file` write the file (DosDupHandle redirects handle 1 to the file).
#[test]
fn vio_window_4os2_cls_and_redirection() {
    let steps = "\
        1500 text echo one > a.txt\n1600 key enter\n\
        1900 text echo two >> a.txt\n2000 key enter\n\
        2300 text hello.exe > c.txt\n2400 key enter\n\
        3300 dump {dump0}\n\
        3400 text cls\n3500 key enter\n\
        3900 dump {dump1}\n\
        4000 quit\n";
    let Some((shots, drive_c, _dir)) = run_4os2_windowed_with(steps, 2, &["samples/hello/hello.exe"]) else { return };
    let read = |name: &str| std::fs::read(drive_c.join(name)).unwrap_or_default();
    assert_eq!(read("a.txt"), b"one\r\ntwo\r\n", "echo > and >> wrote the file");
    assert!(read("c.txt").starts_with(b"Hello from Warpine OS/2 Environment!\r\n"),
            "the child's output went to the file: {:?}", String::from_utf8_lossy(&read("c.txt")));
    // Before cls: the banner on rows 1-3; after: blank rows below the prompt.
    assert!(lit_in_cell_row(&shots[0], 2) > 200, "the banner before cls");
    assert!(lit_in_cell_row(&shots[1], 0) + lit_in_cell_row(&shots[1], 1) > 50, "the prompt after cls");
    for row in 2..8 {
        assert_eq!(lit_in_cell_row(&shots[1], row), 0, "row {row} is blank after cls");
    }
}

/// Mark a rectangle with the mouse (shown inverted, title "Marking: "),
/// Copy it, and Paste it: the marked `md Zk4` runs.
#[test]
fn vio_window_mark_copy_and_paste() {
    // Row 7 holds the prompt `md ` and the typed `Zk4` (4OS2's banner and
    // the `prompt` command above it); mark its first 11 cells.
    let row_y = VIO_CELLS_Y + 7 * 16 + 8;
    let steps = format!("\
        1500 text prompt md$s\n1600 key enter\n\
        2000 text Zk4\n\
        2300 key alt+space\n2400 key a\n\
        2600 drag {x0} {row_y} {x1} {row_y}\n\
        2900 dump {{dump0}}\n\
        3000 key alt+space\n3100 key o\n\
        3300 dump {{dump1}}\n\
        3400 key esc\n\
        3500 key alt+space\n3600 key p\n\
        3900 key enter\n\
        4500 quit\n",
        x0 = VIO_CELLS_X + 4, x1 = VIO_CELLS_X + 10 * 8 + 4);
    let Some((shots, drive_c, _dir)) = run_4os2_windowed(&steps, 2) else { return };
    let white = |s: &(usize, usize, Vec<u8>)| (VIO_CELLS_X..VIO_CELLS_X + 88)
        .filter(|&x| gui_pixel(s, x, row_y) == 0xFF_FFFF).count();
    assert!(white(&shots[0]) > 50, "the marked cells are inverted: {}", white(&shots[0]));
    assert!(white(&shots[1]) < 10, "Copy left Mark mode: {}", white(&shots[1]));
    assert!(has_dir(&drive_c, "Zk4"), "the pasted `md Zk4` ran");
}

/// The tile comparison itself: identical signatures match, a visible change
/// is reported at the right tile, and sub-tolerance noise is ignored.
#[test]
fn gui_tile_signature_detects_local_changes() {
    let (w, h) = (64usize, 48usize);
    let base = vec![200u8; w * h * 3];
    let sig = tile_signature(w, h, &base);
    assert_eq!((sig.len(), sig[0].len()), (3, 4));
    let text = signature_to_text(w, h, &sig);
    let (rw, rh, parsed) = signature_from_text(&text);
    assert_eq!((rw, rh), (w, h));
    assert!(differing_tiles(&sig, &parsed).is_empty());

    // Blacken an 8×8 block inside tile (2, 1): a quarter of the tile.
    let mut changed = base.clone();
    for y in 20..28 { for x in 36..44 { for c in 0..3 { changed[(y * w + x) * 3 + c] = 0; } } }
    assert_eq!(differing_tiles(&tile_signature(w, h, &changed), &sig), vec![(2, 1)]);

    // One slightly different pixel is below the tolerance.
    let mut noisy = base;
    noisy[0] = 190;
    assert!(differing_tiles(&tile_signature(w, h, &noisy), &sig).is_empty());
}

/// Fonts: every Plex family, style and size plus CJK text in four codepages.
#[test]
fn gui_pm_font_test_matches_golden() {
    check_gui_golden("pm_font_test", "samples/pm_font_test/pm_font_test.exe",
        "2500 dump {dump}\n2600 quit\n");
}

/// Built-in controls stay visible after the client paints its background.
#[test]
fn gui_pm_controls_test_matches_golden() {
    check_gui_golden("pm_controls_test", "samples/pm_controls_test/pm_controls_test.exe",
        "2500 dump {dump}\n2600 quit\n");
}

/// Menu bar from FCF_MENU, pull-down open under the File item.
#[test]
fn gui_menu_pulldown_matches_golden() {
    check_gui_golden("pm_menu_pulldown", "samples/pm_menu_dlg_test/pm_menu_dlg_test.exe",
        "1500 click 52 62\n3000 dump {dump}\n3100 quit\n");
}

/// pm_menu_dlg_test replaces its FCF_MENU menu bar the way the PM
/// reference describes — WinDestroyWindow of the FID_MENU window,
/// WinLoadMenu, WM_UPDATEFRAME(FCF_MENU) sent to the frame — and every one
/// of its checks passes (its "Results: n/n passed" line).
#[test]
fn gui_menu_dlg_test_replaces_the_menu_bar() {
    let sample = "samples/pm_menu_dlg_test/pm_menu_dlg_test.exe";
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping gui_menu_dlg_test_replaces_the_menu_bar: KVM or {sample} not available");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("session.script");
    std::fs::write(&script, "2000 quit\n").unwrap();
    let out = run_gui_script(sample, &script, true);
    assert!(!out.contains("FAILED"), "{out}");
    assert!(out.contains("Results: 9/9 passed"), "{out}");
}

/// File → About…: pull-down gone, modal dialog shown and active.
#[test]
fn gui_menu_about_dialog_matches_golden() {
    check_gui_golden("pm_menu_about_dialog", "samples/pm_menu_dlg_test/pm_menu_dlg_test.exe",
        "1500 click 52 62\n2500 click 62 80\n4000 dump {dump}\n4100 quit\n");
}

/// pm_bench: the self-benchmark runs every measurement, reports each with a
/// non-zero count and duration, and runs again when R is pressed. Only the
/// shape of the report is checked; the numbers depend on the machine.
#[test]
fn gui_pm_bench_reports_every_measurement() {
    let sample = "samples/pm_bench/pm_bench.exe";
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping gui_pm_bench_reports_every_measurement: KVM or {sample} not available");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("session.script");
    // One run takes about 3 s; leave room for a loaded test machine.
    std::fs::write(&script, "6000 text r\n12000 quit\n").unwrap();
    let out = run_gui_script(sample, &script, true);

    assert_eq!(out.matches("pm_bench: done").count(), 2, "two complete runs:\n{out}");
    for name in ["GpiBox", "GpiLine", "GpiText", "WinSendMsg", "WinPostMsg", "Paint", "Timer"] {
        let lines: Vec<&str> = out.lines()
            .filter(|l| l.starts_with(&format!("pm_bench: {name}: "))).collect();
        assert_eq!(lines.len(), 2, "{name} reported once per run:\n{out}");
        for line in lines {
            // "pm_bench: <name>: <ops> ops in <ms> ms = ..."
            let words: Vec<&str> = line.split_whitespace().collect();
            let ops: u32 = words[2].parse().unwrap_or_else(|_| panic!("ops in {line:?}"));
            let ms: u32 = words[5].parse().unwrap_or_else(|_| panic!("ms in {line:?}"));
            assert!(ops > 0 && ms > 0, "{line}");
        }
    }
}

/// Run a self-checking sample that prints `<prefix>: <check>: ok` / `FAIL`
/// lines and `<prefix>: done <passed>/<total>`, and require every check to
/// pass. `min_checks` guards against a run that stopped early.
fn assert_sample_checks_pass(sample: &str, prefix: &str, min_checks: usize) {
    assert_scripted_checks_pass(sample, prefix, min_checks, "5000 quit\n");
}

/// [`assert_sample_checks_pass`] for a sample driven by GUI script `steps`.
fn assert_scripted_checks_pass(sample: &str, prefix: &str, min_checks: usize, steps: &str) {
    assert_scripted_checks_pass_env(sample, prefix, min_checks, steps, &[]);
}

/// [`assert_scripted_checks_pass`] with extra environment variables for
/// warpine. Returns whether the sample ran (false: KVM or the sample is
/// not available).
fn assert_scripted_checks_pass_env(sample: &str, prefix: &str, min_checks: usize, steps: &str,
                                   env: &[(&str, &str)]) -> bool {
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping {prefix}: KVM or {sample} not available");
        return false;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("session.script");
    std::fs::write(&script, steps).unwrap();
    let out = run_gui_script_env(sample, &script, true, env);

    let done = format!("{prefix}: done");
    let checks: Vec<&str> = out.lines()
        .filter(|l| l.starts_with(&format!("{prefix}: ")) && !l.starts_with(&done)).collect();
    assert!(checks.len() >= min_checks, "every check reported:\n{out}");
    let failed: Vec<&&str> = checks.iter().filter(|l| !l.ends_with(": ok")).collect();
    assert!(failed.is_empty(), "failed checks: {failed:#?}");
    assert!(out.contains(&format!("{done} {n}/{n}", n = checks.len())), "{out}");
    true
}

/// pm_pel_test: drawing is rasterised on the calling thread, so the guest
/// reads back with GpiQueryPel what GpiSetPel / GpiBox / GpiLine just drew,
/// and XOR / invert / leave-alone mixes combine with the pels already there.
#[test]
fn gui_pm_pel_test_reads_back_pels_and_mixes() {
    assert_sample_checks_pass("samples/pm_pel_test/pm_pel_test.exe", "pm_pel_test", 16);
}

/// pm_textpos_test: GPI text layout — GpiQueryCharStringPos(At) with and
/// without increments, GpiQueryWidthTable, GpiQueryTextBox (TXTBOX_* order,
/// angle, alignment, shear), the character attributes and their queries,
/// GpiCharStringPosAt with CHS_OPAQUE / CHS_CLIP and rotated text read back
/// with GpiQueryPel — checked by the guest itself.
#[test]
fn gui_pm_textpos_test_checks_text_layout() {
    assert_sample_checks_pass("samples/pm_textpos_test/pm_textpos_test.exe", "pm_textpos_test", 36);
}

/// pm_textpos_test's picture: text at six angles around a mark, sheared,
/// top to bottom, right to left, spaced by an increment vector and
/// underscored, centred and struck out on an opaque box.
#[test]
fn gui_pm_textpos_test_matches_golden() {
    check_gui_golden("pm_textpos_test", "samples/pm_textpos_test/pm_textpos_test.exe",
        "3000 dump {dump}\n3100 quit\n");
}

/// pm_rect_text_test: the rectangle helpers, WinDrawText (counts, extent,
/// word break, erasing) and GpiPolyLine (from the current position, read
/// back with GpiQueryPel), checked by the guest itself.
#[test]
fn gui_pm_rect_text_test_checks_rectangles_draw_text_and_polylines() {
    assert_sample_checks_pass("samples/pm_rect_text_test/pm_rect_text_test.exe", "pm_rect_text_test", 39);
}

/// pm_paint_test: update rectangles (WinInvalidateRect / WinValidateRect /
/// WinQueryUpdateRect / WinBeginPaint, one WM_PAINT for two invalidations,
/// none once valid), WinScrollWindow (moved pels read back with
/// GpiQueryPel, uncovered area, SW_INVALIDATERGN, clip rectangle,
/// SW_SCROLLCHILDREN) and WinDrawBorder (sides, interior, DB_DESTINVERT,
/// DB_STANDARD, errors) — checked by the guest itself.
#[test]
fn gui_pm_paint_test_checks_update_rectangles_scrolling_and_borders() {
    assert_sample_checks_pass("samples/pm_paint_test/pm_paint_test.exe", "pm_paint_test", 59);
}

/// The picture of pm_paint_test: borders (plain, standard, dialog,
/// inverted) and a stripe pattern whose lower half was scrolled.
#[test]
fn gui_pm_paint_test_matches_golden() {
    check_gui_golden("pm_paint_test", "samples/pm_paint_test/pm_paint_test.exe",
        "2500 dump {dump}\n2600 quit\n");
}

/// pm_bitmap_test: a memory PS drawing into a bitmap, GpiBitBlt (1:1,
/// stretched, ROP_DSTINVERT), GpiQueryBitmapBits, GpiLoadBitmap from an
/// RT_BITMAP resource with WinDrawBitmap, the same-id RLE8 bitmap of a
/// resource DLL (DosLoadModule + GpiLoadBitmap with its module handle),
/// logical colour tables, line types and bitmap selection rules — checked
/// from the guest side.
#[test]
fn gui_pm_bitmap_test_blits_bitmaps() {
    assert_sample_checks_pass("samples/pm_bitmap_test/pm_bitmap_test.exe", "pm_bitmap_test", 29);
}

/// pm_shapes_test: arcs (GpiFullArc / GpiPartialArc / GpiPointArc and arc
/// parameters), area patterns in fills and ROP_PATCOPY blits, and cosmetic /
/// geometric line widths — checked from the guest side.
#[test]
fn gui_pm_shapes_test_draws_arcs_patterns_and_wide_lines() {
    assert_sample_checks_pass("samples/pm_shapes_test/pm_shapes_test.exe", "pm_shapes_test", 29);
}

/// The picture half of pm_shapes_test: circle, ellipse, pie wedge, pattern
/// swatches and line widths.
#[test]
fn gui_pm_shapes_test_matches_golden() {
    check_gui_golden("pm_shapes_test", "samples/pm_shapes_test/pm_shapes_test.exe",
        "2500 dump {dump}\n2600 quit\n");
}

/// pm_xform_test: model transforms (GpiTranslate / GpiScale / GpiRotate,
/// TRANSFORM_ADD), GpiConvert / GpiConvertWithMatrix, the page viewport,
/// PU_LOENGLISH page units, clip rectangles and viewing limits in world
/// coordinates, GpiSavePS / GpiRestorePS / GpiResetPS and attribute
/// bundles — checked from the guest side with GpiQueryPel and GpiConvert.
#[test]
fn gui_pm_xform_test_transforms_and_saves_state() {
    assert_sample_checks_pass("samples/pm_xform_test/pm_xform_test.exe", "pm_xform_test", 39);
}

/// The picture half of pm_xform_test: a rosette of rotated squares, an
/// ellipse scaled from a circle, a sheared box, a star of rotated lines and
/// a centimetre grid in a PU_LOMETRIC presentation space.
#[test]
fn gui_pm_xform_test_matches_golden() {
    check_gui_golden("pm_xform_test", "samples/pm_xform_test/pm_xform_test.exe",
        "2500 dump {dump}\n2600 quit\n");
}

/// pm_region_test: regions (create / combine / query / offset / equal,
/// GpiQueryRegionRects, paint and frame), clipping (clip rectangles, clip
/// region ownership, clip box, clip path), areas (alternate / winding /
/// exclusive edges), paths (fill / outline / stroke / path to region),
/// polygons, curves and window update regions — checked from the guest side
/// with GpiQueryPel.
#[test]
fn gui_pm_region_test_clips_fills_and_strokes() {
    assert_sample_checks_pass("samples/pm_region_test/pm_region_test.exe", "pm_region_test", 53);
}

/// The picture half of pm_region_test: pentagrams (alternate / winding),
/// a patterned and framed region, stripes clipped to a circle path and to
/// a region with a hole, a stroked zigzag and a spline.
#[test]
fn gui_pm_region_test_matches_golden() {
    check_gui_golden("pm_region_test", "samples/pm_region_test/pm_region_test.exe",
        "2500 dump {dump}\n2600 quit\n");
}

/// Keyboard steps of the pm_dialog_test session up to the open dialog with a
/// name typed and the second list item selected.
const DIALOG_TEST_FILL_IN: &str = "1500 key ctrl+d\n2500 text Warp\n2800 key tab\n3000 key down\n3200 key down\n";

/// pm_dialog_test: a modal dialog driven from the keyboard only — Ctrl+D
/// from the frame's accelerator table (FCF_ACCELTABLE) opens it, typing goes
/// to the entry field that has the focus, Tab moves to the list box, Enter
/// presses the default push button (WM_COMMAND DID_OK, CMDSRC_PUSHBUTTON),
/// Esc cancels, Right moves within the OK / Cancel group, Alt+N (the ~Name
/// label) focuses the entry field and Alt+C presses ~Cancel, and Alt+F4
/// (system accelerator) closes the main window through
/// WM_SYSCOMMAND(SC_CLOSE) and WM_CLOSE. The sample also checks WinPeekMsg,
/// the disabled owner and that WinDlgBox destroys the dialog.
#[test]
fn gui_pm_dialog_test_is_driven_by_the_keyboard() {
    let steps = format!("{DIALOG_TEST_FILL_IN}3700 key enter\n\
        4500 key ctrl+d\n5100 text ro\n5300 key esc\n\
        6000 key ctrl+d\n6800 key tab\n7000 key tab\n7200 key right\n7500 key enter\n\
        8200 key ctrl+d\n9000 key tab\n9200 key tab\n9400 key alt+n\n9700 text Zz\n10000 key alt+c\n\
        10800 key ctrl+d\n11600 text Hello\n11800 key shift+left\n11900 key shift+left\n12000 text p!\n\
        12200 key shift+home\n12300 key ctrl+insert\n12400 key end\n12500 key shift+insert\n12700 key enter\n\
        13500 key alt+f4\n17000 quit\n");
    assert_scripted_checks_pass("samples/pm_dialog_test/pm_dialog_test.exe", "pm_dialog_test", 37, &steps);
}

/// pm_list_test driven with the mouse and keyboard: two clicks and Down,
/// Space in the multiple-selection list, a click, Shift+click and Ctrl+click
/// in the extended-selection list, a double-click and Enter in the single
/// list (row y = 123 + 16 * row).
const LIST_TEST_STEPS: &str = "700 click 100 123\n1000 click 100 155\n1300 key down\n1500 key space\n\
    1800 click 300 139\n2100 shift+click 300 171\n2400 ctrl+click 300 155\n\
    2700 dblclick 500 187\n3000 key enter\n";

/// List box selection modes, LN_ENTER and MLE word wrap, checked by the
/// guest: LM_SELECTITEM / LM_QUERYSELECTION on a multiple selection, the
/// clicks and keys of [`LIST_TEST_STEPS`], LIT_CURSOR, MLM_SETWRAP. F3 makes
/// the sample report and exit, checking the message queue lifecycle on the
/// way out (one queue per thread, WinDestroyMsgQueue, a new queue).
#[test]
fn gui_pm_list_test_selects_with_mouse_and_keyboard() {
    let steps = format!("{LIST_TEST_STEPS}3600 key f3\n8000 quit\n");
    assert_scripted_checks_pass("samples/pm_list_test/pm_list_test.exe", "pm_list_test", 17, &steps);
}

/// Built-in controls follow their frame when it is dragged: the
/// pm_list_test window is moved by (+100, +100) by its title bar, then
/// [`LIST_TEST_STEPS`] are replayed 100 px further right and down. Every
/// click must land on the same list rows as without the move.
#[test]
fn gui_controls_follow_a_dragged_window() {
    let moved = shift_script(LIST_TEST_STEPS, 1000, 100, 100);
    let steps = format!("500 drag 300 42 400 142\n{moved}4600 key f3\n9000 quit\n");
    assert_scripted_checks_pass("samples/pm_list_test/pm_list_test.exe", "pm_list_test", 17, &steps);
}

/// `steps` (one GUI script step per line) delayed by `dt` ms with the
/// coordinates of every click moved by `(dx, dy)`.
fn shift_script(steps: &str, dt: u64, dx: i32, dy: i32) -> String {
    steps.lines().map(|line| {
        let mut words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        words[0] = (words[0].parse::<u64>().unwrap() + dt).to_string();
        if words[1].ends_with("click") {
            words[2] = (words[2].parse::<i32>().unwrap() + dx).to_string();
            words[3] = (words[3].parse::<i32>().unwrap() + dy).to_string();
        }
        words.join(" ") + "\n"
    }).collect()
}

/// The lists of pm_list_test after [`LIST_TEST_STEPS`]: several items
/// highlighted in the multiple and extended lists, one in the single list,
/// and the MLE text wrapped at word boundaries.
#[test]
fn gui_pm_list_test_matches_golden() {
    check_gui_golden("pm_list_test", "samples/pm_list_test/pm_list_test.exe",
        &format!("{LIST_TEST_STEPS}3600 dump {{dump}}\n3700 quit\n"));
}

/// The pm_dialog_test dialog filled in: entry field text, list selection,
/// default and focus marks, dialog chrome.
#[test]
fn gui_pm_dialog_test_matches_golden() {
    check_gui_golden("pm_dialog_test", "samples/pm_dialog_test/pm_dialog_test.exe",
        &format!("{DIALOG_TEST_FILL_IN}4000 dump {{dump}}\n4100 quit\n"));
}

/// pm_button_test session up to the last state of its dialog: Space on the
/// auto check box, Space twice on the auto three-state button, Space on the
/// plain check box, Tab into the auto-radio group (onto the button checked
/// in WM_INITDLG) and Space, Down twice (past the BS_NOCURSORSELECT button,
/// wrapping to the first), Tab into the plain radio group and Space, Up and
/// Space, Alt+B, a click on the Right radio button (desktop (240, 468)).
const BUTTON_TEST_STEPS: &str = "1600 key space\n1700 key tab\n1800 key space\n1900 key space\n\
    2000 key tab\n2100 key space\n2200 key tab\n2300 key space\n2400 key down\n2500 key down\n\
    2600 key tab\n2700 key space\n2800 key up\n2900 key space\n3000 key alt+b\n3200 click 240 468\n";

/// pm_button_test: check boxes and radio buttons — BM_SETCHECK /
/// BM_QUERYCHECK / BM_QUERYCHECKINDEX, auto check boxes toggling, an auto
/// three-state button cycling, auto-radio buttons unchecking each other, the
/// arrow keys checking auto-radio buttons unless BS_NOCURSORSELECT, Tab
/// landing on a group's checked radio button, plain buttons left to their
/// owner, BN_CLICKED for each selection, then Enter.
#[test]
fn gui_pm_button_test_checks_boxes_and_radio_buttons() {
    let steps = format!("{BUTTON_TEST_STEPS}3500 key enter\n7000 quit\n");
    assert_scripted_checks_pass("samples/pm_button_test/pm_button_test.exe", "pm_button_test", 54, &steps);
}

/// The pm_button_test dialog after [`BUTTON_TEST_STEPS`]: unchecked, ticked
/// and indeterminate check boxes, radio buttons with and without the dot,
/// the focus mark around the Right label.
#[test]
fn gui_pm_button_test_matches_golden() {
    check_gui_golden("pm_button_test", "samples/pm_button_test/pm_button_test.exe",
        &format!("{BUTTON_TEST_STEPS}3400 dump {{dump}}\n3500 quit\n"));
}

/// Answers to the first three message boxes of pm_msgbox_test: Enter on the
/// MB_DEFBUTTON2 default, Esc, a click on Yes (desktop (468, 416): the box
/// is centred on the desktop).
const MSGBOX_TEST_STEPS: &str = "1500 key enter\n2200 key esc\n2900 click 468 416\n";

/// WinMessageBox is a modal PM dialog: the default button, Esc for Cancel and
/// clicks pick its result, while the owner's frame is disabled and its
/// window procedure keeps getting timer messages. Then Enter on OK, and in
/// the last box a click on Help (desktop (600, 416); the box stays open)
/// and one on the close button (desktop (643, 334)), which ends it with
/// MBID_CANCEL although it has no Cancel button.
#[test]
fn gui_pm_msgbox_test_answers_message_boxes() {
    let steps = format!("{MSGBOX_TEST_STEPS}3700 key enter\n4400 click 600 416\n5100 click 643 334\n9000 quit\n");
    assert_scripted_checks_pass("samples/pm_msgbox_test/pm_msgbox_test.exe", "pm_msgbox_test", 9, &steps);
}

/// pm_destroy_test: WinDestroyWindow sends WM_DESTROY to the window and then
/// to its children while they all still exist, a frame destroys the
/// windows it and its children own, nested WinDestroyWindow calls from a
/// WM_DESTROY handler send nothing twice, and WinDlgBox destroys its
/// dialog with WM_DESTROY before it returns.
#[test]
fn gui_pm_destroy_test_sends_wm_destroy() {
    assert_sample_checks_pass("samples/pm_destroy_test/pm_destroy_test.exe", "pm_destroy_test", 23);
}

/// The fourth box of pm_msgbox_test: the information icon left of two
/// lines of text, the OK button with the default outline, centred on the
/// desktop over its inactive owner.
#[test]
fn gui_pm_msgbox_test_matches_golden() {
    check_gui_golden("pm_msgbox_test", "samples/pm_msgbox_test/pm_msgbox_test.exe",
        &format!("{MSGBOX_TEST_STEPS}3600 dump {{dump}}\n3700 quit\n"));
}

/// pm_error_test: failing PM calls leave their PMERR_* code (with
/// SEVERITY_ERROR) for WinGetLastError, which resets it; a later success
/// keeps it; each thread has its own last error. Covers PMERR_INVALID_HWND
/// from the window, dialog-item and message calls, PMERR_PARAMETER_OUT_OF_RANGE,
/// PMERR_INVALID_FLAG and PMERR_INVALID_HACCEL. WinSetSysValue sets the
/// caret blink rate (SV_CURSORRATE) and rejects a rate of 0 / a bad desktop.
#[test]
fn gui_pm_error_test_reports_last_errors() {
    assert_sample_checks_pass("samples/pm_error_test/pm_error_test.exe", "pm_error_test", 114);
}

/// pm_profile_test: PMSHAPI profiles — PrfWriteProfileString / Data,
/// PrfQueryProfileString / Int / Size / Data with defaults, truncation,
/// enumeration and deletion, PrfOpenProfile / PrfCloseProfile and their
/// PMERR_* codes — and the HELPMGR calls (help instance, association,
/// help tables, HM_* messages accepted). Drive C: is a temporary
/// directory; the profiles left there are Warpine's text format.
#[test]
fn gui_pm_profile_test_reads_and_writes_profiles() {
    let drive = tempfile::tempdir().expect("tempdir");
    let xdg = drive.path().to_str().unwrap();
    if !assert_scripted_checks_pass_env("samples/pm_profile_test/pm_profile_test.exe",
                                        "pm_profile_test", 65, "5000 quit\n",
                                        &[("XDG_DATA_HOME", xdg)]) {
        return;
    }
    let drive_c = drive.path().join("warpine/drive_c");
    let user = std::fs::read_to_string(drive_c.join("OS2/OS2.INI")).expect("user profile");
    assert!(user.starts_with("; warpine-profile 1\n"), "{user}");
    assert!(user.contains("[\"PrfTest\"]\n\"Greeting\" = \"hello\\0\"\n"), "{user}");
    assert!(user.contains("\"Blob\" = \"\\0\\x01\\x02\\xff\\\"\\0\"\n"), "{user}");
    assert!(!user.contains("PrfTemp"), "the deleted application is gone: {user}");
    let private = std::fs::read_to_string(drive_c.join("PRFTEST.INI")).expect("private profile");
    assert!(private.contains("[\"Private\"]\n\"Key\" = \"private value\\0\""), "{private}");
    // Only the profiles: the temporary files were renamed over them.
    let mut names: Vec<_> = std::fs::read_dir(&drive_c).unwrap().flatten()
        .map(|e| e.file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["OS2", "PRFTEST.INI"]);
}

/// nest_test: a program that starts itself without end stops at the
/// process nesting limit (`inherit::MAX_PROCESS_DEPTH`, 8): the deepest copy
/// gets ERROR_NO_PROC_SLOTS (89) from DosExecPgm, every copy above it
/// reports its child, and the first copy's exit code counts the copies
/// below it. No Warpine process is left running.
#[test]
fn test_nest_test_stops_at_the_nesting_limit() {
    if !kvm_available() || !Path::new("samples/nest_test/nest_test.exe").exists() { return; }
    let (out, code) = run_sample("samples/nest_test/nest_test.exe", 120);
    assert_eq!(code, 8, "exit code (copies started below the first)\nstdout:\n{out}");
    assert_eq!(out.matches("nest_test: DosExecPgm refused rc=89").count(), 1, "{out}");
    assert_eq!(out.matches("nest_test: child ended with").count(), 8, "{out}");
}

/// Whether `exe` was linked with the start-up code of the Open Watcom
/// snapshots that read the entry stack one slot off (Open Watcom #1649,
/// fixed 2026-08-11): `__OS2Main`'s `mov eax,[ebp+0x10]; mov ebx,[ebp+4];
/// push eax`. Such a program sees no argv and no environment on any system.
fn built_with_shifted_ow_startup(exe: &str) -> bool {
    const SHIFTED_ARGS: [u8; 7] = [0x8B, 0x45, 0x10, 0x8B, 0x5D, 0x04, 0x50];
    std::fs::read(exe).is_ok_and(|b| b.windows(SHIFTED_ARGS.len()).any(|w| w == SHIFTED_ARGS))
}

/// Run an ok/FAIL sample and check that every check passed (its last line is
/// "<name>: done <passed>/<total>" and its exit code the failures).
fn assert_all_checks_pass(sample: &str, env: &[(&str, &str)]) {
    if !kvm_available() || !Path::new(sample).exists() { return; }
    let (out, code) = run_sample_args(sample, &[], 60, env);
    assert_eq!(code, 0, "exit code\n{out}");
    assert!(!out.contains("FAIL"), "{out}");
    let done = out.lines().find_map(|l| l.split_once(": done ").map(|(_, n)| n.to_owned()))
        .unwrap_or_else(|| panic!("no done line\n{out}"));
    let (passed, total) = done.split_once('/').expect("passed/total");
    assert_eq!(passed, total, "{out}");
}

/// infoseg_test: DosGetInfoSeg's GINFOSEG (time kept current, date, time
/// zone, version) and LINFOSEG (ids), through their selectors.
#[test]
fn test_info_segments() {
    assert_all_checks_pass("samples/infoseg_test/infoseg_test.exe", &[("TZ", "JST-9")]);
}

/// fileattr_test: FILE_READONLY set and cleared by path, by handle and with
/// DosSetFileMode; a read-only file cannot be written or deleted.
#[test]
fn test_file_attributes() {
    assert_all_checks_pass("samples/fileattr_test/fileattr_test.exe", &[]);
}

/// datetime_test: dates after 2038 (ADR-0039). The session starts at
/// 2040-03-01 (`WARPINE_CLOCK`) in a fixed zone nine hours east of UTC
/// (`TZ=JST-9`, no daylight saving): DosGetDateTime / QSV_TIME / the C
/// runtime's time() past 2038, DosSetDateTime to 2110 (a child started
/// afterwards inherits it), file times up to 2107 and WARPINE.DLL's 64-bit
/// time calls. A second run starts on the host's clock.
#[test]
fn test_dates_after_2038() {
    const SAMPLE: &str = "samples/datetime_test/datetime_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let env = [("TZ", "JST-9"), ("WARPINE_CLOCK", "2040-03-01T12:00:00")];
    let (out, code) = run_sample_args(SAMPLE, &["2040"], 120, &env);
    assert_eq!(code, 0, "exit code\n{out}");
    assert!(!out.contains("FAIL"), "{out}");
    assert!(out.contains("datetime_test: timezone=-540"), "minutes west of UTC\n{out}");
    assert!(out.contains("datetime_test: start year from WARPINE_CLOCK: ok"), "{out}");
    assert!(out.contains("datetime_test: child started afterwards sees 2110: ok"), "{out}");
    let done = out.lines().find(|l| l.starts_with("datetime_test: done ")).expect("done line");
    let (passed, total) = done["datetime_test: done ".len()..].split_once('/').unwrap();
    assert_eq!(passed, total, "{out}");

    let (out, code) = run_sample_args(SAMPLE, &[], 120, &[("TZ", "JST-9")]);
    assert_eq!(code, 0, "host clock\n{out}");
    assert!(out.contains("datetime_test: done ") && !out.contains("FAIL"), "{out}");
}

/// argv_test, started with arguments: the C runtime's argc / argv and
/// environment come from the entry stack ([esp+12] environment, [esp+16]
/// command line) in the first process and in its DosExecPgm children; every
/// process of the session has its own id, which its parent also uses
/// (EXEC_ASYNC's result, DosWaitChild, DosKillProcess) and which its
/// children see as `pib_ulppid`.
#[test]
fn test_argv_and_process_ids() {
    const SAMPLE: &str = "samples/argv_test/argv_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    if built_with_shifted_ow_startup(SAMPLE) {
        eprintln!("skipped: {SAMPLE} was built with an Open Watcom snapshot older than 2026-08-11 \
                   (#1649); rebuild it with a newer one (vendor/setup_watcom.sh --refresh)");
        return;
    }
    let (out, code) = run_sample_args(SAMPLE, &["alpha", "beta", "gamma"], 120, &[]);
    assert_eq!(code, 0, "exit code\n{out}");
    assert!(!out.contains("FAIL"), "{out}");
    assert!(out.contains("argv_test: done, 0 failed"), "{out}");
    for line in ["argv_test: parent argv[1]=alpha", "argv_test: parent argv[3]=gamma",
                 "argv_test: child argv[1]=child", "argv_test: child argv[5]=three",
                 "argv_test: childsync argv[3]=x", "argv_test: grandchild argv[1]=grandchild",
                 "argv_test: ok child environment from DosExecPgm",
                 "argv_test: parent argv[0]=C:\\ARGV_TEST.EXE"] {
        assert!(out.contains(line), "missing {line:?}\n{out}");
    }
    // "argv_test: <who> pid=<pid> ppid=<ppid> argc=<n>"
    let ids: std::collections::HashMap<&str, (u32, u32)> = out.lines().filter_map(|l| {
        let rest = l.strip_prefix("argv_test: ")?;
        let (who, rest) = rest.split_once(" pid=")?;
        let (pid, rest) = rest.split_once(" ppid=")?;
        let (ppid, _) = rest.split_once(' ')?;
        Some((who, (pid.parse().ok()?, ppid.parse().ok()?)))
    }).collect();
    assert_eq!(ids.len(), 4, "{out}");
    let (parent, child, sync, grand) = (ids["parent"], ids["child"], ids["childsync"], ids["grandchild"]);
    assert_eq!(parent, (42, 0), "the first process of a session");
    assert_eq!(child.1, parent.0);
    assert_eq!(sync.1, parent.0);
    assert_eq!(grand.1, sync.0, "a grandchild's parent is the child that started it");
    let mut pids = vec![parent.0, child.0, sync.0, grand.0];
    pids.sort_unstable();
    pids.dedup();
    assert_eq!(pids.len(), 4, "process ids are unique in the session: {ids:?}");
    assert!(out.contains(&format!("child's own pid is the one DosExecPgm returned ({}, exit {})",
                                  child.0, child.0 & 0xFF)), "{out}");
}

/// threads_limit_test with `THREADS=70` in drive C's CONFIG.SYS: the limit
/// covers the whole session — DosCreateThread fails with
/// ERROR_MAX_THRDS_REACHED (164) after 69 threads besides thread 1, a child
/// running beside 10 threads of its parent gets 59, DosExecPgm fails with
/// ERROR_NO_PROC_SLOTS (89) when no slot is left, and the slots of ended
/// threads and processes come back. No Warpine process or accounting
/// record is left.
#[test]
fn test_threads_limit_from_config_sys() {
    const SAMPLE: &str = "samples/threads_limit_test/threads_limit_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let data = tempfile::tempdir().unwrap();
    let run = tempfile::tempdir().unwrap();
    let drive_c = data.path().join("warpine/drive_c");
    std::fs::create_dir_all(&drive_c).unwrap();
    let config = b"REM THREADS=500\r\nthreads = 300\r\nTHREADS=70\r\n";
    std::fs::write(drive_c.join("CONFIG.SYS"), config).unwrap();
    let (out, code) = run_sample_env(SAMPLE, 120, &[
        ("XDG_DATA_HOME", data.path().to_str().unwrap()),
        ("XDG_RUNTIME_DIR", run.path().to_str().unwrap()),
    ]);
    assert_eq!(code, 0, "stdout:\n{out}");
    assert!(!out.contains("FAIL"), "{out}");
    for line in ["ok DosCreateThread at the limit rc=164", "ok created threads=69",
                 "ok child created threads=59", "ok children started=2",
                 "ok DosExecPgm without a slot rc=89",
                 "ok synchronous DosExecPgm without a slot rc=89",
                 "ok child after the slots came back result=7", "ok created again threads=69"] {
        assert!(out.contains(line), "missing {line:?}:\n{out}");
    }
    // CONFIG.SYS is only read; the session directory is gone.
    assert_eq!(std::fs::read(drive_c.join("CONFIG.SYS")).unwrap(), config);
    assert_eq!(std::fs::read_dir(run.path()).unwrap().count(), 0);
}

/// `WARPINE_THREADS` overrides CONFIG.SYS (and is clamped to 64 … 4095).
#[test]
fn test_threads_limit_from_the_environment() {
    const SAMPLE: &str = "samples/threads_limit_test/threads_limit_test.exe";
    if !kvm_available() || !Path::new(SAMPLE).exists() { return; }
    let run = tempfile::tempdir().unwrap();
    let (out, code) = run_sample_env(SAMPLE, 120, &[
        ("WARPINE_THREADS", "66"),
        ("XDG_RUNTIME_DIR", run.path().to_str().unwrap()),
    ]);
    assert_eq!(code, 0, "stdout:\n{out}");
    assert!(out.contains("ok created threads=65"), "{out}");
    assert!(out.contains("ok child created threads=55"), "{out}");
    let (out, _) = run_sample_env(SAMPLE, 120, &[
        ("WARPINE_THREADS", "5"),
        ("XDG_RUNTIME_DIR", run.path().to_str().unwrap()),
    ]);
    assert!(out.contains("ok created threads=63"), "clamped to 64:\n{out}");
}

/// A process joining a session whose threads are all in use ends with
/// ERROR_NO_PROC_SLOTS (89) before running guest code; a record of a dead
/// process does not count.
#[test]
fn test_threads_limit_refuses_a_process_without_a_slot() {
    use std::os::unix::fs::PermissionsExt;
    if !kvm_available() { return; }
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("s");
    std::fs::create_dir(&session).unwrap();
    std::fs::set_permissions(&session, std::fs::Permissions::from_mode(0o700)).unwrap();
    // This test process, holding every slot.
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let start = stat[stat.rfind(')').unwrap() + 1..].split_whitespace().nth(19).unwrap().to_string();
    let me = std::process::id();
    let record = session.join(format!("p-{me}"));
    std::fs::write(&record, format!("{me} {start} 4095 1\n")).unwrap();
    let dir_s = session.to_str().unwrap();
    let (out, code) = run_sample_env("samples/hello/hello.exe", 30, &[("WARPINE_THREAD_DIR", dir_s)]);
    assert_eq!(code, 89, "stdout:\n{out}");
    assert!(!out.contains("Hello"), "{out}");
    // The same record with another start time is a dead process.
    std::fs::write(&record, format!("{me} 1 4095 1\n")).unwrap();
    let (out, code) = run_sample_env("samples/hello/hello.exe", 30, &[("WARPINE_THREAD_DIR", dir_s)]);
    assert_eq!(code, 0, "stdout:\n{out}");
    assert!(out.contains("Hello"), "{out}");
    assert!(!record.exists(), "the stale record is removed");
}

/// pm_switch_dde_test: the switch list (FCF_TASKLIST entry, WinAddSwitchEntry
/// … WinSwitchToProgram, the entry gone with its frame), atom tables
/// (string and integer atoms, usage counts, the system table), a DDE
/// conversation between two frames of the process (initiate / respond,
/// request → data, execute → ack, terminate), WinStartApp of a second copy
/// of the program (parameters, environment, WM_APPTERMINATENOTIFY with its
/// return code), WinTerminateApp of a sleeping copy, and the MSG.DLL calls
/// (DosInsertMessage, DosPutMessage, DosGetMessage, DosQueryMessageCP) —
/// checked by the guest itself.
#[test]
fn gui_pm_switch_dde_test_checks_switch_list_atoms_dde_and_start_app() {
    assert_scripted_checks_pass("samples/pm_switch_dde_test/pm_switch_dde_test.exe", "pm_switch_dde_test",
                                90, "20000 quit\n");
}

/// pm_window_test: WinIsWindowVisible (the window and all its parents),
/// WinQueryWindowTextLength, WinSetParent (to another window, the desktop
/// and back; not under a descendant), WinSetOwner, and WinQueryWindowPtr /
/// WinSetWindowPtr (application words shared with WinQueryWindowULong,
/// QWP_PFNWP, the cbWindowData bound).
#[test]
fn gui_pm_window_test_checks_the_window_tree_calls() {
    assert_sample_checks_pass("samples/pm_window_test/pm_window_test.exe", "pm_window_test", 36);
}

/// pm_frame_accel_test: frame scroll bars from FCF_VERTSCROLL and
/// WinCreateFrameControls (on a WC_FRAME window with an FID_CLIENT
/// client), WinCalcFrameRect, WM_QUERYFRAMECTLCOUNT / WM_FORMATFRAME /
/// WM_CALCFRAMERECT, WinCreateAccelTable / WinCopyAccelTable /
/// WinQueryAccelTable / WinDestroyAccelTable, WinLoadMessage /
/// WinLoadString from wrc bundles, WinQueryVersion, WinFlashWindow,
/// WinSetWindowBits, WinSetMultWindowPos and WinSetDlgItemShort /
/// WinQueryDlgItemShort.
#[test]
fn gui_pm_frame_accel_test_checks_frames_accelerators_and_window_calls() {
    assert_sample_checks_pass("samples/pm_frame_accel_test/pm_frame_accel_test.exe", "pm_frame_accel_test", 72);
}

/// pm_query_test: window lookups (WinMapWindowPoints, WinWindowFromPoint,
/// WinIsChild, WinMultWindowFromIDs, WinEnumDlgItem, WinQueryClassName /
/// WinQueryClassInfo …), system colours (WinSetSysColors changes the title
/// bar and SYSCLR_WINDOW, WM_SYSCOLORCHANGE arrives) and input state: a
/// Shift+click in the client (frame at (100, 100), 500×400: screen
/// (400, 600) is client (298, 67), away from the buttons) and Shift+F5 —
/// WinGetKeyState as of the message being processed, WinQueryMsgPos at the
/// click, WinGetPhysKeyState of the Shift key after its second release.
#[test]
fn gui_pm_query_test_checks_lookups_input_state_and_system_colours() {
    let steps = "1000 shift+click 400 600\n1500 key shift+f5\n6000 quit\n";
    assert_scripted_checks_pass("samples/pm_query_test/pm_query_test.exe", "pm_query_test", 83, steps);
}

/// pm_pointer_test: system pointer handles and copies, WinSetPointer /
/// WinQueryPointer (NULLHANDLE removes the pointer), the WinShowPointer
/// display level, WinLoadPointer from RT_POINTER resources (a
/// black-and-white pointer and a bitmap array written by make_ptr.py),
/// WinDestroyPointer, and the arrow set by WinDefWindowProc on
/// WM_MOUSEMOVE. The host cursor follows; the dummy video driver has none,
/// which the program must not notice.
#[test]
fn gui_pm_pointer_test_sets_and_loads_pointers() {
    assert_sample_checks_pass("samples/pm_pointer_test/pm_pointer_test.exe", "pm_pointer_test", 42);
}

/// pm_typeahead_test: keyboard input is routed when the program reads it.
/// The client is busy for 600 ms after 'n' and then focuses the first entry
/// field; "ab" typed meanwhile goes there, and "cd" typed after a click on
/// the second entry field (client (20, 330, 200, 24): desktop y 161–185)
/// goes to that one. The sample also checks the order of a queue: posted
/// messages, then WM_PAINT, then WM_TIMER.
#[test]
fn gui_pm_typeahead_test_routes_keys_when_they_are_read() {
    let steps = "1000 text n\n1100 text ab\n1300 click 120 173\n1400 text cd\n6000 quit\n";
    assert_scripted_checks_pass("samples/pm_typeahead_test/pm_typeahead_test.exe", "pm_typeahead_test", 5, steps);
}

/// pm_inputorder_test: pointer input is routed when the program reads it
/// (ADR-0051). Frame A (desktop x 40–340) is active, frame B (x 400–700)
/// beside it, both at PM y 400–600 (script y 168–367). While the program is
/// busy, 'a', a click on B's client and 'b' arrive: 'a' goes to A, B is
/// activated when the click is read, before it, and 'b' goes to B. Then a
/// click at the same point and 'c' wait while the program swaps the frames:
/// the click goes to A, now under the pointer, and activates it.
#[test]
fn gui_pm_inputorder_test_routes_clicks_when_they_are_read() {
    let steps = "1500 text a\n1600 click 550 287\n1700 text b\n4500 click 550 287\n4600 text c\n14000 quit\n";
    assert_scripted_checks_pass("samples/pm_inputorder_test/pm_inputorder_test.exe", "pm_inputorder_test", 3, steps);
}

/// pm_twoproc_test: two PM programs — a parent and the child it starts —
/// on the one desktop of the session's desktop server (ADR-0052). Frame A
/// (the parent, desktop x 40–340) and frame B (the child, x 400–700) at PM
/// y 400–600 (script y 168–367). A click on A, then 'w' (A stays busy 2 s),
/// 'x', a click on B and 'y': B gets the click and 'y' only after A has read
/// 'x'. A click on A and 'z' take activation back; 'q' ends both.
#[test]
fn gui_pm_twoproc_test_shares_one_desktop_with_input_in_order() {
    let steps = "3000 click 190 287\n3300 text w\n3500 text x\n3700 click 550 287\n3900 text y\n\
                 6500 click 190 287\n6700 text z\n7300 text q\n15000 quit\n";
    assert_scripted_checks_pass("samples/pm_twoproc_test/pm_twoproc_test.exe", "pm_twoproc_test", 6, steps);
}

/// pm_twoproc_test's desktop with both programs' frames: A active (its
/// title bar navy), B inactive.
#[test]
fn gui_pm_twoproc_test_matches_golden() {
    check_gui_golden("pm_twoproc_test", "samples/pm_twoproc_test/pm_twoproc_test.exe",
        "3000 click 550 287\n3300 click 190 287\n4000 dump {dump}\n4100 quit\n");
}

/// The desktop kept in the process (`WARPINE_DESKTOP=local`, no desktop
/// server) shows what the server shows: pm_minmax_test's golden picture and
/// pm_inputorder_test's checks.
#[test]
fn gui_in_process_desktop_matches_the_server() {
    let local: &[(&str, &str)] = &[("WARPINE_DESKTOP", "local")];
    check_gui_golden_env("pm_minmax_test", "samples/pm_minmax_test/pm_minmax_test.exe",
        "1500 dblclick 300 42\n2000 dblclick 300 8\n2500 key alt+f9\n3000 dump {dump}\n3100 quit\n", local);
    let steps = "1500 text a\n1600 click 550 287\n1700 text b\n4500 click 550 287\n4600 text c\n14000 quit\n";
    assert_scripted_checks_pass_env("samples/pm_inputorder_test/pm_inputorder_test.exe", "pm_inputorder_test", 3,
                                    steps, local);
}

/// pm_minmax_test: frames are minimized, maximized and restored through
/// WinSetWindowPos and WM_SYSCOMMAND (checked from the guest), then with
/// the mouse and the system keys. Frame A is the default 640×480 frame at
/// desktop (32, 32): title bar y 32–51, minimize button x 618–633, maximize
/// x 636–651, system menu x 35–50. Maximized it covers the desktop with its
/// 2-px border outside (title bar y −2–17, restore button x 990–1005);
/// minimized it is a 160×22 bar in the first icon slot, desktop (4, 742).
#[test]
fn gui_pm_minmax_test_minimizes_maximizes_and_restores() {
    let steps = "\
        1500 dblclick 300 42\n\
        2000 dblclick 300 8\n\
        2500 key alt+f10\n\
        3000 key alt+f5\n\
        3500 key alt+f9\n\
        4000 dblclick 80 752\n\
        4500 click 626 42\n\
        5000 dblclick 80 752\n\
        5500 click 644 42\n\
        6000 click 998 8\n\
        6200 click 903 527\n\
        6500 dblclick 43 42\n\
        9000 quit\n";
    assert_scripted_checks_pass("samples/pm_minmax_test/pm_minmax_test.exe", "pm_minmax_test", 60, steps);
}

/// pm_minmax_test with frame A minimized by Alt+F9: a bordered title bar in
/// the first icon slot at the bottom-left of the desktop, frame B unchanged.
#[test]
fn gui_pm_minmax_test_matches_golden() {
    check_gui_golden("pm_minmax_test", "samples/pm_minmax_test/pm_minmax_test.exe",
        "1500 dblclick 300 42\n2000 dblclick 300 8\n2500 key alt+f9\n3000 dump {dump}\n3100 quit\n");
}

/// pm_sysmenu_test: a frame's system menu opened by a click on its icon,
/// Shift+Esc and Alt+Space; items chosen by mnemonic and by a click arrive
/// as WM_SYSCOMMAND(SC_*, CMDSRC_MENU, fPointer) — one the client
/// intercepts —; Alt+F7 / Alt+F8 and the menu's Move / Size move and size
/// the frame with the arrow keys (Esc restores); WinPopupMenu with
/// PU_POSITIONONITEM puts the item under the pointer and a click on it
/// posts WM_COMMAND. The frame is the default one at desktop (32, 32):
/// system-menu icon around (43, 42); after the keyboard moves it is at
/// (40, 40) and its menu's Close item at y 165–180.
#[test]
fn gui_pm_sysmenu_test_opens_the_system_menu_and_tracks_with_the_keyboard() {
    let steps = "\
        1000 click 43 42\n\
        1500 key m\n\
        1800 key right\n\
        1900 key right\n\
        2000 key down\n\
        2100 key enter\n\
        2500 key alt+f8\n\
        2800 key right\n\
        2900 key right\n\
        3000 key down\n\
        3100 key down\n\
        3200 key esc\n\
        3600 key alt+f7\n\
        3900 key left\n\
        4000 key enter\n\
        4400 key shift+esc\n\
        4700 key x\n\
        5100 key alt+space\n\
        5400 key s\n\
        5700 key right\n\
        5800 key right\n\
        5900 key enter\n\
        6300 rclick 310 300\n\
        6700 click 316 300\n\
        7100 key alt+space\n\
        7400 click 60 173\n\
        9000 quit\n";
    assert_scripted_checks_pass("samples/pm_sysmenu_test/pm_sysmenu_test.exe", "pm_sysmenu_test", 44, steps);
}

/// pm_sysmenu_test with its system menu open (a click on the icon): Restore
/// and Hide greyed, accelerator texts right-aligned, Window list greyed.
#[test]
fn gui_pm_sysmenu_test_matches_golden() {
    check_gui_golden("pm_sysmenu_test", "samples/pm_sysmenu_test/pm_sysmenu_test.exe",
        "1000 click 43 42\n1300 dump {dump}\n1400 quit\n");
}

/// pm_scroll_test: the client (default frame at desktop (32, 32), title bar
/// only, client from (34, 52): PM y = 511 − screen y) holds a vertical
/// scroll bar at client (400, 100) 16 × 200 — screen x 434–449, y 212–411 —
/// and a horizontal one at (50, 40) 200 × 16 — screen x 84–283, y 456–471.
/// Clicks on the up and down arrows and the shaft, a slider drag 70 pels
/// down (position 40 → 90) and clicks on the horizontal bar's right arrow
/// and shaft arrive as WM_VSCROLL / WM_HSCROLL (SB_LINE*, SB_PAGE*,
/// SB_SLIDERTRACK, SB_SLIDERPOSITION, SB_ENDSCROLL), which the client
/// answers with SBM_SETPOS; SBM_* messages, SBCDATA and the PM text cursor
/// calls are checked from the guest side.
const SCROLL_TEST_STEPS: &str = "\
    1000 click 441 218\n\
    1300 click 441 404\n\
    1600 click 441 250\n\
    1900 drag 441 290 441 360\n\
    2400 click 276 463\n\
    2700 click 200 463\n";

#[test]
fn gui_pm_scroll_test_scrolls_with_the_mouse() {
    let steps = format!("{SCROLL_TEST_STEPS}3200 key alt+f4\n6000 quit\n");
    assert_scripted_checks_pass("samples/pm_scroll_test/pm_scroll_test.exe", "pm_scroll_test", 83, &steps);
}

/// pm_scroll_test after the session: the vertical slider (thumb size 20 of
/// 120) near the bottom, the horizontal one at 4 of 10, and the frame
/// cursor inverted over the blue box.
#[test]
fn gui_pm_scroll_test_matches_golden() {
    check_gui_golden("pm_scroll_test", "samples/pm_scroll_test/pm_scroll_test.exe",
        &format!("{SCROLL_TEST_STEPS}3000 dump {{dump}}\n3100 quit\n"));
}

/// pm_frame_scroll_test: a frame at desktop (100, 200), 400 × 300, with
/// FCF_VERTSCROLL | FCF_HORZSCROLL and a sizing border (screen y = 767 − PM
/// y). The vertical bar lies at screen x 484–499 (its right edge under the
/// border) from y 296 (up arrow) to 542 (down arrow), the horizontal bar at
/// screen y 550–565 from x 104 (left arrow) to 483 (right arrow), the
/// client above it from screen (102, 268) to (483, 549). Clicks on the
/// vertical bar's arrows, a slider drag 100 pels down, a click on the
/// shaft below the slider, on the horizontal bar's right arrow and shaft,
/// and on the client at its (50, 50) arrive as WM_VSCROLL / WM_HSCROLL at
/// the client (and WM_BUTTON1DOWN in client coordinates); the client's
/// drawing lies above the horizontal bar (GpiQueryPel inside and outside
/// it), all checked from the guest side.
const FRAME_SCROLL_STEPS: &str = "\
    1000 click 490 296\n\
    1300 click 490 542\n\
    1600 drag 490 313 490 413\n\
    2100 click 490 527\n\
    2400 click 476 557\n\
    2700 click 300 557\n\
    3000 click 152 499\n";

#[test]
fn gui_pm_frame_scroll_test_takes_the_pointer_on_frame_scroll_bars() {
    let steps = format!("{FRAME_SCROLL_STEPS}3400 key alt+f4\n6000 quit\n");
    assert_scripted_checks_pass("samples/pm_frame_scroll_test/pm_frame_scroll_test.exe",
                                "pm_frame_scroll_test", 68, &steps);
}

/// pm_frame_scroll_test after the session: the client (pale grey, a blue
/// box at its (0, 0), a red one at its top-right corner) above the
/// horizontal frame scroll bar and left of the vertical one, the sliders
/// where the clicks left them.
#[test]
fn gui_pm_frame_scroll_test_matches_golden() {
    check_gui_golden("pm_frame_scroll_test", "samples/pm_frame_scroll_test/pm_frame_scroll_test.exe",
        &format!("{FRAME_SCROLL_STEPS}3300 dump {{dump}}\n3400 quit\n"));
}

/// pm_child_dialog_test: a dialog loaded with the client as parent is
/// embedded in it (ADR-0042). The frame is at desktop (100, 200), 400 × 300,
/// the client's (0, 0) at screen (102, 567); the dialog at client (20, 20),
/// 300 × 200, its push button at dialog (10, 10), 100 × 30, its entry field
/// at (10, 150), 200 × 24. The steps click the button, click the entry field
/// and type, click the dialog's own area at its (250, 100); the guest checks
/// the messages, focus, text, activation and destruction.
const CHILD_DIALOG_STEPS: &str = "\
    1000 click 182 522\n\
    1300 click 232 385\n\
    1500 text abc\n\
    1800 click 372 447\n";

#[test]
fn gui_pm_child_dialog_test_embeds_a_dialog_in_a_client() {
    let steps = format!("{CHILD_DIALOG_STEPS}2100 key alt+f4\n4000 quit\n");
    assert_scripted_checks_pass("samples/pm_child_dialog_test/pm_child_dialog_test.exe",
                                "pm_child_dialog_test", 15, &steps);
}

/// pm_child_dialog_test's picture: the dialog face with its entry field
/// ("abc") and push button inside the grey client, below the title bar.
#[test]
fn gui_pm_child_dialog_test_matches_golden() {
    check_gui_golden("pm_child_dialog_test", "samples/pm_child_dialog_test/pm_child_dialog_test.exe",
        &format!("{CHILD_DIALOG_STEPS}2000 dump {{dump}}\n2100 quit\n"));
}

/// pm_notebook_test: a WC_NOTEBOOK (client (8, 8), 380 × 250, major tabs on
/// the right) with three pages, each an embedded dialog. The steps click the
/// "First" major tab, the "Sub" minor tab, "First" again, and the entry
/// field on page 1, and type; the guest checks the BKM_* messages, that
/// BKN_PAGESELECTED arrives (with its PAGESELECTNOTIFY) before the turn
/// returns, the shown and hidden page windows, the typed text and
/// BKN_PAGEDELETED.
const NOTEBOOK_STEPS: &str = "\
    1000 click 446 325\n\
    1300 click 144 545\n\
    1600 click 446 325\n\
    1900 click 175 422\n\
    2100 text xyz\n";

#[test]
fn gui_pm_notebook_test_turns_pages_and_notifies() {
    let steps = format!("{NOTEBOOK_STEPS}2400 key alt+f4\n4500 quit\n");
    assert_scripted_checks_pass("samples/pm_notebook_test/pm_notebook_test.exe", "pm_notebook_test", 42, &steps);
}

/// pm_notebook_test's picture: page 1 (a label and "xyz" in its entry
/// field) with the status line and page buttons below it, the "First" and
/// "Second" major tabs on the right, the "Sub" minor tab at the bottom.
#[test]
fn gui_pm_notebook_test_matches_golden() {
    check_gui_golden("pm_notebook_test", "samples/pm_notebook_test/pm_notebook_test.exe",
        &format!("{NOTEBOOK_STEPS}2300 dump {{dump}}\n2400 quit\n"));
}

/// pm_container_test: a WC_CONTAINER (client (8, 8), 380 × 250, extended
/// selection) with five records in the detail view (a title, column
/// titles, name / size / date columns). The guest checks record and field
/// allocation, insertion orders, queries, CNRINFO, CM_SEARCHSTRING,
/// CN_EMPHASIS before CM_SETRECORDEMPHASIS returns and
/// PMERR_RECORD_CURRENTLY_INSERTED; the steps click row 1, shift+click
/// row 3 (three selected), double click row 0 (CN_ENTER from the mouse),
/// press Down and Enter (CN_ENTER from the keyboard).
const CONTAINER_STEPS: &str = "\
    1000 click 140 376\n\
    1300 shift+click 140 412\n\
    1600 dblclick 140 358\n\
    1900 key down\n\
    2100 key enter\n";

#[test]
fn gui_pm_container_test_selects_and_notifies() {
    let steps = format!("{CONTAINER_STEPS}2400 key alt+f4\n4500 quit\n");
    assert_scripted_checks_pass("samples/pm_container_test/pm_container_test.exe", "pm_container_test", 47, &steps);
}

/// pm_container_test's picture: the "Files" title, the column titles over
/// a separator, five rows with column separators, alpha.txt selected and
/// cursored.
#[test]
fn gui_pm_container_test_matches_golden() {
    check_gui_golden("pm_container_test", "samples/pm_container_test/pm_container_test.exe",
        &format!("{CONTAINER_STEPS}2300 dump {{dump}}\n2400 quit\n"));
}

/// pm_cnrtree_test: a WC_CONTAINER in the tree view with twenty items and a
/// folder of three children inserted out of order. The guest checks that
/// CM_SORTRECORD calls its comparison function (through call chains) and
/// sorts each level, that a CNRINFO sort function places a new record and
/// that CM_EXPANDTREE / CM_COLLAPSETREE notify before they return; the
/// steps click the folder's expander and the scroll bar's down arrow twice
/// (CN_EXPANDTREE, CN_SCROLL).
const CNRTREE_STEPS: &str = "\
    1000 click 119 320\n\
    1300 click 481 550\n\
    1600 click 481 550\n";

#[test]
fn gui_pm_cnrtree_test_sorts_expands_and_scrolls() {
    let steps = format!("{CNRTREE_STEPS}2000 key alt+f4\n4500 quit\n");
    assert_scripted_checks_pass("samples/pm_cnrtree_test/pm_cnrtree_test.exe", "pm_cnrtree_test", 27, &steps);
}

/// pm_cnrtree_test's picture: the expanded tree scrolled by two rows
/// (child b on top, indented), the items below, the scroll bar on the right.
#[test]
fn gui_pm_cnrtree_test_matches_golden() {
    check_gui_golden("pm_cnrtree_test", "samples/pm_cnrtree_test/pm_cnrtree_test.exe",
        &format!("{CNRTREE_STEPS}1900 dump {{dump}}\n2000 quit\n"));
}

/// pm_cnricon_test: the container's icon view with system icons, CM_FILTER
/// through a guest filter function, CM_ARRANGE, the record-rectangle
/// queries and CM_OPENEDIT / CM_CLOSEEDIT in the guest; the steps edit the
/// folder's text directly (Alt+click, typing, a click on white space keeps
/// it through CN_REALLOCPSZ) and discard a second edit with Esc.
const CNRICON_STEPS: &str = "\
    1000 alt+click 157 417\n\
    1300 text Archive\n\
    1600 click 410 329\n\
    1900 alt+click 277 417\n\
    2100 text zzz\n\
    2300 key esc\n";

#[test]
fn gui_pm_cnricon_test_filters_arranges_and_edits() {
    let steps = format!("{CNRICON_STEPS}2600 key alt+f4\n5000 quit\n");
    assert_scripted_checks_pass("samples/pm_cnricon_test/pm_cnricon_test.exe", "pm_cnricon_test", 34, &steps);
}

/// pm_cnricon_test's picture: two rows of icons (folder, program, file,
/// several files, application) with their texts, the folder renamed
/// "Archive".
#[test]
fn gui_pm_cnricon_test_matches_golden() {
    check_gui_golden("pm_cnricon_test", "samples/pm_cnricon_test/pm_cnricon_test.exe",
        &format!("{CNRICON_STEPS}2500 dump {{dump}}\n2600 quit\n"));
}

/// pm_subclass_test: WinUpdateWindow and the WinBeginPaint clip, a child
/// window's presentation space, WinShowWindow invalidating, subclassing an
/// entry field and a standard frame (WM_QUERYFRAMECTLCOUNT /
/// WM_FORMATFRAME placing a tool bar), WM_QUERYDLGCODE / WM_MATCHMNEMONIC,
/// a thread's windows going with it; the steps drive a dialog whose own
/// "Dial" control takes a mnemonic (Alt+Z, asked through WM_MATCHMNEMONIC)
/// and passes Tab and Alt+O on to the dialog through WinDefWindowProc.
const SUBCLASS_STEPS: &str = "\
    1500 key alt+z\n\
    1800 key tab\n\
    2100 key shift+tab\n";

#[test]
fn gui_pm_subclass_test_paints_subclasses_and_formats() {
    let steps = format!("{SUBCLASS_STEPS}2400 key alt+o\n5000 quit\n");
    assert_scripted_checks_pass("samples/pm_subclass_test/pm_subclass_test.exe", "pm_subclass_test", 28, &steps);
}

/// pm_subclass_test's picture: the frame with the subclass's "Tools" bar
/// above the blue client, the green child window and the entry field, and
/// the dialog with the cyan Dial control focused.
#[test]
fn gui_pm_subclass_test_matches_golden() {
    check_gui_golden("pm_subclass_test", "samples/pm_subclass_test/pm_subclass_test.exe",
        &format!("{SUBCLASS_STEPS}2300 dump {{dump}}\n2400 quit\n"));
}

/// pm_drag_test: drag and drop within a process. The guest checks the
/// PMDRAG data calls (string handles, DRAGINFO items, DrgVerify*); the
/// steps drag with button 2 from the client onto a container (WM_BEGINDRAG,
/// DrgDrag, CN_DRAGOVER, CN_DROP inserting a record, DrgFreeDraginfo
/// refused during the drop) and from that record onto the client's left
/// half (CN_INITDRAG, DOR_NEVERDROP, DM_DRAGLEAVE, NULLHANDLE).
const DRAG_STEPS: &str = "\
    1000 rdrag 152 467 352 467\n\
    1800 rdrag 322 319 130 520\n";

#[test]
fn gui_pm_drag_test_drags_and_drops() {
    let steps = format!("{DRAG_STEPS}2700 key alt+f4\n5000 quit\n");
    assert_scripted_checks_pass("samples/pm_drag_test/pm_drag_test.exe", "pm_drag_test", 32, &steps);
}

/// pm_drag_test's picture: the dark source half of the client and the
/// container holding the dropped "dropped.txt" record.
#[test]
fn gui_pm_drag_test_matches_golden() {
    check_gui_golden("pm_drag_test", "samples/pm_drag_test/pm_drag_test.exe",
        &format!("{DRAG_STEPS}2600 dump {{dump}}\n2700 quit\n"));
}

/// pm_charbundle_test: GpiSetAttrs / GpiQueryAttrs with a CHARBUNDLE set
/// and report the character mode, angle, shear, direction, alignment,
/// extra and break extra that the single GpiSetChar* calls use, reject
/// invalid values and reset to the defaults.
#[test]
fn gui_pm_charbundle_test_maps_the_character_bundle() {
    assert_sample_checks_pass("samples/pm_charbundle_test/pm_charbundle_test.exe", "pm_charbundle_test", 37);
}

/// pm_modal_filter_test: message-filter hooks in the modal loops of
/// WinMessageBox (MSGF_MESSAGEBOX: a swallowed, a changed message) and the
/// standard dialogs (MSGF_DIALOGBOX); FDM_FILTER / FDM_ERROR / FDM_VALIDATE
/// and FNTM_FILTERLIST / FNTM_FACENAMECHANGED sent to the dialog
/// procedures, which drive the dialogs themselves.
#[test]
fn gui_pm_modal_filter_test_filters_modal_loops_and_answers_standard_dialogs() {
    assert_sample_checks_pass("samples/pm_modal_filter_test/pm_modal_filter_test.exe", "pm_modal_filter_test", 29);
}

/// pm_combo_spin_test (same frame placement as pm_scroll_test: client from
/// screen (34, 52), PM y = 511 − screen y): clicks the drop-down arrow of
/// the CBS_DROPDOWN combination box (client (162..180, 348..370)) and the
/// second item of its dropped list (client y 314..330), the up arrow of the
/// numeric spin button (client (104..120, 212..224)), the down arrow of the
/// day spin button (client (244..260, 200..212)) and the second 24-pel item
/// of the owner-drawn list box (client y 130..154), then drops the
/// combination box again. Notifications (CBN_*, SPBN_*, LN_SELECT,
/// WM_MEASUREITEM / WM_DRAWITEM, BN_PAINT) and the values reached are
/// checked from the guest side.
const COMBO_SPIN_STEPS: &str = "\
    1000 click 205 152\n\
    1400 click 94 189\n\
    1800 click 146 293\n\
    2100 click 286 305\n\
    2400 click 94 369\n\
    2800 click 205 152\n";

#[test]
fn gui_pm_combo_spin_test_drops_picks_spins_and_owner_draws() {
    let steps = format!("{COMBO_SPIN_STEPS}3300 key alt+f4\n6000 quit\n");
    assert_scripted_checks_pass("samples/pm_combo_spin_test/pm_combo_spin_test.exe", "pm_combo_spin_test", 59, &steps);
}

/// pm_msg_hook_test: input, send, message-filter and help hooks on the
/// calling queue (a swallowed, a changed message; SMHSTRUCT; an inter-task
/// send from a second thread seen by WinInSendMsg), WinPostQueueMsg,
/// WinBroadcastMsg, WinWaitMsg, WinWaitEventSem and WinRequestMutexSem.
#[test]
fn gui_pm_msg_hook_test_calls_hooks_and_broadcasts() {
    assert_sample_checks_pass("samples/pm_msg_hook_test/pm_msg_hook_test.exe", "pm_msg_hook_test", 68);
}

/// pm_combo_spin_test after the session: "Green" picked and the list
/// dropped again over the client, the spin buttons at 6 and "Sun", the
/// owner-drawn colour swatches with item 1 selected (drawn by the owner)
/// and item 3 drawn by the list box, the cyan user button.
#[test]
fn gui_pm_combo_spin_test_matches_golden() {
    check_gui_golden("pm_combo_spin_test", "samples/pm_combo_spin_test/pm_combo_spin_test.exe",
        &format!("{COMBO_SPIN_STEPS}3200 dump {{dump}}\n3300 quit\n"));
}

/// pm_stddlg_test: the Open dialog starts on "C:\STDDLG\*.TXT" with the
/// filter selected in its entry field: "beta.txt" replaces it and Enter
/// presses OK; the font dialog then has the focus on its name list: Down
/// picks the next family (IBM Plex Serif) and Enter is OK. Keys typed
/// before a dialog is up wait for it (type-ahead).
const STDDLG_STEPS: &str = "\
    2000 text beta.txt\n\
    2400 key enter\n\
    3400 key down\n\
    3800 key enter\n";

/// pm_stddlg_test: WinFileDlg (subclassed by a dialog procedure that
/// passes on to WinDefFileDlgProc) returns the typed file in its on-disk
/// case and WinFontDlg the picked family and an FATTRS GpiCreateLogFont
/// takes; the spooler reports no queues or printers, SplQm* / DevOpenDC
/// (OD_QUEUED / OD_DIRECT) / DevEscape / DevPostDeviceModes fail with the
/// documented errors — checked from the guest side.
#[test]
fn gui_pm_stddlg_test_picks_a_file_and_a_font() {
    let steps = format!("{STDDLG_STEPS}9000 quit\n");
    assert_scripted_checks_pass("samples/pm_stddlg_test/pm_stddlg_test.exe", "pm_stddlg_test", 43, &steps);
}

/// pm_stddlg_test: the Open dialog, centred on the desktop over the
/// program's window — file name, current directory, type and drive lists,
/// the directory and file lists, OK / Cancel.
#[test]
fn gui_pm_stddlg_test_open_dialog_matches_golden() {
    check_gui_golden("pm_stddlg_test", "samples/pm_stddlg_test/pm_stddlg_test.exe",
        "1800 dump {dump}\n1900 quit\n");
}

/// pm_presparam_test: presentation parameters (WinSetPresParam /
/// WinQueryPresParam with inheritance and colour indices /
/// WinRemovePresParam, the PRESPARAMS of WinCreateWindow and of a dialog
/// template, WM_PRESPARAMCHANGED), built-in controls drawn in them, the
/// SS_* static styles, class styles (CS_SIZEREDRAW, CS_SYNCPAINT,
/// CS_CLIPCHILDREN), WM_ERASEBACKGROUND and WinLockWindowUpdate.
#[test]
fn gui_pm_presparam_test_checks_presentation_parameters_and_painting() {
    assert_sample_checks_pass("samples/pm_presparam_test/pm_presparam_test.exe", "pm_presparam_test", 55);
}

/// pm_presparam_test once checked: red Courier text, the rectangle,
/// halftone and frame statics, the information icon and the green bitmap,
/// the yellow button and cyan entry field, the magenta group box, and the
/// dialog with its red label in the template's smaller font.
#[test]
fn gui_pm_presparam_test_matches_golden() {
    check_gui_golden("pm_presparam_test", "samples/pm_presparam_test/pm_presparam_test.exe",
                     "2000 dump {dump}\n2100 quit\n");
}

/// pm_slider_valueset_test (same frame placement as pm_scroll_test: client
/// from screen (34, 52), PM y = 511 − screen y): slider 10 at client
/// (20, 330) 300 × 60 has its shaft across client y 360, its two buttons at
/// client x 22..54 and increments 40 pels apart from client x 56. The
/// session clicks the shaft right of the arm (→ 40), drags the arm from
/// client x 100 to 207 (snaps to 160), clicks the right button (→ 200),
/// presses Left (→ 160) and Shift+Right (→ the detent at 180); clicks item
/// (2, 3) of value set 20 (client (20, 40) 240 × 100, 2 × 4 items) and
/// presses Right, double-clicks its item (1, 1) and clicks the leftmost item
/// of the right-to-left value set 21 (column 3). Notifications (SLN_*,
/// VN_*) and owner drawing (WM_DRAWITEM for the slider arm and a value set
/// item) are checked from the guest side.
const SLIDER_VALUESET_STEPS: &str = "\
    1000 click 204 151\n\
    1300 drag 134 151 241 151\n\
    1800 click 79 151\n\
    2100 key left\n\
    2300 key shift+right\n\
    2600 click 198 444\n\
    2900 key right\n\
    3200 dblclick 80 394\n\
    3600 click 324 451\n";

#[test]
fn gui_pm_slider_valueset_test_drags_steps_and_selects() {
    let steps = format!("{SLIDER_VALUESET_STEPS}4000 key alt+f4\n7000 quit\n");
    assert_scripted_checks_pass("samples/pm_slider_valueset_test/pm_slider_valueset_test.exe",
                                "pm_slider_valueset_test", 68, &steps);
}

/// pm_slider_valueset_test after the session: slider 10 on its detent at
/// 180 with the ribbon strip up to it, the progress slider at 6 of 10, the
/// owner-drawn red arm, the vertical slider with scale 2 on the left, the
/// colour grid with item (1, 1) selected, the owner-drawn colour-index
/// item and "Three" selected with the dotted cursor.
#[test]
fn gui_pm_slider_valueset_test_matches_golden() {
    check_gui_golden("pm_slider_valueset_test", "samples/pm_slider_valueset_test/pm_slider_valueset_test.exe",
        &format!("{SLIDER_VALUESET_STEPS}3900 dump {{dump}}\n4000 quit\n"));
}

/// pm_codepage_test: PM text follows the message queue's codepage
/// (WinSetCp, including CP1004, which DosSetProcessCp refuses), WinQueryCpList
/// lists the queue codepages, and a presentation space starts with its queue's
/// codepage (GpiQueryCp) until GpiSetCp changes it.
#[test]
fn gui_pm_codepage_test_follows_the_queue_codepage() {
    assert_sample_checks_pass("samples/pm_codepage_test/pm_codepage_test.exe", "pm_codepage_test", 15);
}

/// pm_audio_test: DosBeep and MMPM/2 waveaudio played through the main-thread
/// SDL3 audio service (dummy audio driver): playback blocks for the file's
/// length with "wait", "notify" posts MM_MCINOTIFY, aliases and the
/// mciSendCommand parameter blocks follow the toolkit headers.
#[test]
fn gui_pm_audio_test_plays_through_the_audio_service() {
    assert_sample_checks_pass("samples/pm_audio_test/pm_audio_test.exe", "pm_audio_test", 17);
}

// Colours the compositor draws with (`src/gui/sdl_renderer.rs`).
const GUI_DESKTOP_BG: u32 = 0x40_8040;
const GUI_TITLE_ACTIVE: u32 = 0x00_0080;
const GUI_WHITE: u32 = 0xFF_FFFF;

/// `drag` on the title bar moves the frame: the default 640×480 frame sits at
/// desktop (32, 32) with a 20-px title bar, and is dragged by (+200, +200).
/// Checked on the pixels themselves, before and after, so the test fails if
/// the drag does nothing, lands elsewhere, or leaves the old image behind.
#[test]
fn gui_drag_title_bar_moves_window() {
    check_drag_title_bar_moves_window(&[]);
}

fn check_drag_title_bar_moves_window(env: &[(&str, &str)]) {
    let sample = "samples/pm_menu_dlg_test/pm_menu_dlg_test.exe";
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping gui_drag_title_bar_moves_window: KVM or {sample} not available");
        return;
    }
    let dumps = run_gui_sample_dumps_env(sample,
        "1500 dump {dump0}\n1600 drag 300 42 500 242\n2600 dump {dump1}\n2700 quit\n", 2, env);
    let (before, after) = (&dumps[0], &dumps[1]);

    // Before: title bar across y 32..52, client below, bare desktop elsewhere.
    assert_eq!(gui_pixel(before, 600, 42), GUI_TITLE_ACTIVE, "title bar at its initial position");
    assert_eq!(gui_pixel(before, 600, 242), GUI_WHITE, "client area under the initial title bar");
    assert_eq!(gui_pixel(before, 800, 600), GUI_DESKTOP_BG, "desktop right of / below the frame");

    // After: the frame spans x 232..872 from y 232; its title bar is 200 px lower.
    assert_eq!(gui_pixel(after, 600, 242), GUI_TITLE_ACTIVE, "title bar at the new position");
    assert_eq!(gui_pixel(after, 800, 242), GUI_TITLE_ACTIVE, "title bar reaches the new right edge");
    assert_eq!(gui_pixel(after, 800, 600), GUI_WHITE, "client area moved with the frame");
    for (x, y) in [(600, 42), (100, 42), (100, 400), (600, 220), (220, 600)] {
        assert_eq!(gui_pixel(after, x, y), GUI_DESKTOP_BG,
            "({x}, {y}) should show the desktop once the frame has left");
    }
}

/// `drag` on a sizing border resizes the frame: the right edge of the
/// FCF_SIZEBORDER frame of pm_controls_test (x 32..672) is pulled 100 px to
/// the right, so title bar and client extend to x ≈ 772 and no further.
#[test]
fn gui_drag_sizing_border_resizes_window() {
    check_drag_sizing_border_resizes_window(&[]);
}

fn check_drag_sizing_border_resizes_window(env: &[(&str, &str)]) {
    let sample = "samples/pm_controls_test/pm_controls_test.exe";
    if !kvm_available() || !Path::new(sample).exists() {
        eprintln!("skipping gui_drag_sizing_border_resizes_window: KVM or {sample} not available");
        return;
    }
    let dumps = run_gui_sample_dumps_env(sample,
        "1500 dump {dump0}\n1600 drag 669 300 769 300\n2600 dump {dump1}\n2700 quit\n", 2, env);
    let (before, after) = (&dumps[0], &dumps[1]);

    assert_eq!(gui_pixel(before, 600, 42), GUI_TITLE_ACTIVE, "title bar before the resize");
    assert_eq!(gui_pixel(before, 700, 42), GUI_DESKTOP_BG, "desktop right of the frame");
    assert_eq!(gui_pixel(before, 720, 300), GUI_DESKTOP_BG, "desktop right of the frame");

    assert_eq!(gui_pixel(after, 700, 42), GUI_TITLE_ACTIVE, "title bar grew to the right");
    assert_ne!(gui_pixel(after, 720, 300), GUI_DESKTOP_BG, "frame now covers the widened area");
    assert_eq!(gui_pixel(after, 790, 300), GUI_DESKTOP_BG, "frame grew by the drag distance only");
    assert_eq!(gui_pixel(after, 790, 42), GUI_DESKTOP_BG, "frame grew by the drag distance only");
    // The left edge did not move.
    assert_eq!(gui_pixel(after, 20, 300), GUI_DESKTOP_BG, "left edge stayed in place");
    assert_eq!(gui_pixel(after, 200, 42), gui_pixel(before, 200, 42), "left part of the title bar unchanged");
}

/// Editing built-in controls end to end: click an entry field and type
/// (including a shifted and a non-ASCII character), Home then more text at the
/// start, then click the multi-line field and add a line. The sample ends
/// itself 3 s after start, so every step happens before that.
#[test]
fn gui_typing_into_controls_matches_golden() {
    check_gui_golden("pm_controls_typing", "samples/pm_controls_test/pm_controls_test.exe",
        "700 click 230 300\n900 text  Hello, W\u{f6}rld!\n1300 key home\n1400 text >>\n\
         1600 click 200 470\n1800 key enter\n1900 text three\n2500 dump {dump}\n2600 quit\n");
}

/// A selection in an entry field: text typed after a click, then Shift+Home
/// selects back to the start and Shift+Right gives one character back; the
/// selected characters are drawn highlighted.
#[test]
fn gui_entry_field_selection_matches_golden() {
    check_gui_golden("pm_controls_selection", "samples/pm_controls_test/pm_controls_test.exe",
        "700 click 230 300\n900 key end\n1000 text  selected\n1300 key shift+home\n1400 key shift+right\n2500 dump {dump}\n2600 quit\n");
}

// ── Layered compositor ───────────────────────────────────────────────────────
//
// `WARPINE_COMPOSITOR=layered` keeps one texture per frame and draws them with
// the SDL renderer (on the dummy driver, SDL's software renderer). It must
// show the same desktop as the software compositor the golden files come
// from: frames, chrome, an open pull-down, overlapping frames, moves and
// resizes.

// The layered compositor draws only an in-process desktop: the desktop
// server (ADR-0052) composites in software.
const LAYERED: &[(&str, &str)] = &[("WARPINE_COMPOSITOR", "layered"), ("WARPINE_DESKTOP", "local")];

#[test]
fn gui_layered_compositor_matches_golden_fonts() {
    check_gui_golden_env("pm_font_test", "samples/pm_font_test/pm_font_test.exe",
        "2500 dump {dump}\n2600 quit\n", LAYERED);
}

#[test]
fn gui_layered_compositor_matches_golden_pulldown() {
    check_gui_golden_env("pm_menu_pulldown", "samples/pm_menu_dlg_test/pm_menu_dlg_test.exe",
        "1500 click 52 62\n3000 dump {dump}\n3100 quit\n", LAYERED);
}

#[test]
fn gui_layered_compositor_matches_golden_dialog() {
    check_gui_golden_env("pm_menu_about_dialog", "samples/pm_menu_dlg_test/pm_menu_dlg_test.exe",
        "1500 click 52 62\n2500 click 62 80\n4000 dump {dump}\n4100 quit\n", LAYERED);
}

#[test]
fn gui_layered_compositor_matches_golden_dialog_controls() {
    check_gui_golden_env("pm_dialog_test", "samples/pm_dialog_test/pm_dialog_test.exe",
        &format!("{DIALOG_TEST_FILL_IN}4000 dump {{dump}}\n4100 quit\n"), LAYERED);
}

/// Built-in controls painted over the client's background.
#[test]
fn gui_layered_compositor_matches_golden_controls() {
    check_gui_golden_env("pm_controls_test", "samples/pm_controls_test/pm_controls_test.exe",
        "2500 dump {dump}\n2600 quit\n", LAYERED);
}

/// Typing changes small parts of a window that already has its texture,
/// so only the changed rows are uploaded (`Surface::take_changes`).
#[test]
fn gui_layered_compositor_matches_golden_typing() {
    check_gui_golden_env("pm_controls_typing", "samples/pm_controls_test/pm_controls_test.exe",
        "700 click 230 300\n900 text  Hello, W\u{f6}rld!\n1300 key home\n1400 text >>\n\
         1600 click 200 470\n1800 key enter\n1900 text three\n2500 dump {dump}\n2600 quit\n", LAYERED);
}

#[test]
fn gui_layered_compositor_matches_golden_shapes() {
    check_gui_golden_env("pm_shapes_test", "samples/pm_shapes_test/pm_shapes_test.exe",
        "2500 dump {dump}\n2600 quit\n", LAYERED);
}

/// WinScrollWindow moves pels inside a surface the layered compositor
/// already holds as a texture: the moved area must reach it (partial upload).
#[test]
fn gui_layered_compositor_matches_golden_paint() {
    check_gui_golden_env("pm_paint_test", "samples/pm_paint_test/pm_paint_test.exe",
        "2500 dump {dump}\n2600 quit\n", LAYERED);
}

/// Scroll bars repainted in place and the PM text cursor laid over a
/// texture that already exists (partial uploads with the cursor applied).
#[test]
fn gui_layered_compositor_matches_golden_scroll_bars_and_cursor() {
    check_gui_golden_env("pm_scroll_test", "samples/pm_scroll_test/pm_scroll_test.exe",
        &format!("{SCROLL_TEST_STEPS}3000 dump {{dump}}\n3100 quit\n"), LAYERED);
}

#[test]
fn gui_layered_compositor_moves_windows() {
    check_drag_title_bar_moves_window(LAYERED);
}

#[test]
fn gui_layered_compositor_resizes_windows() {
    check_drag_sizing_border_resizes_window(LAYERED);
}
