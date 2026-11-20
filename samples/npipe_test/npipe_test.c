/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * npipe_test.c - named pipes, pipes passed to child processes, and
 * DosGiveSharedMem
 *
 * The program is its own child: run without arguments it is the parent,
 * (the arguments are read from the PIB's command line),
 * which starts copies of itself through DosExecPgm with "child <mode>":
 *
 *   child anon <h>   write to inherited anonymous-pipe handle <h>
 *   child stdin      read standard input (a pipe) to its end; the exit
 *                    code is the number of bytes read
 *   child stdout     write to standard output (a pipe)
 *   child env        exit code 0 when its environment holds what the
 *                    parent passed
 *   child named      talk to the parent's named pipe \PIPE\NPTEST\XPROC:
 *                    DosOpen + DosWrite + DosRead, then DosWaitNPipe +
 *                    DosCallNPipe; the exit code is the number of failures
 *
 * Copied as FIND.EXE it is a minimal FIND: it prints the lines of its
 * standard input that contain its argument (the shell pipeline test).
 *
 * The parent checks, one line each on stdout:
 *
 *   npipe_test: <name>: ok
 *   npipe_test: <name>: FAIL got <value> want <value>
 *
 * followed by "npipe_test: done <passed>/<total>".
 */
#define INCL_DOSPROCESS
#define INCL_DOSNMPIPES
#define INCL_DOSFILEMGR
#define INCL_DOSMEMMGR
#define INCL_DOSERRORS
#define INCL_DOSQUEUES
#define INCL_DOSMISC
#include <os2.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define XPROC_PIPE  "\\PIPE\\NPTEST\\XPROC"
#define LOCAL_PIPE  "\\PIPE\\NPTEST\\LOCAL"
#define SHMEM_NAME  "\\SHAREMEM\\NPTEST"

static int n_checks = 0;
static int n_passed = 0;

static void check(const char *name, ULONG got, ULONG want)
{
    n_checks++;
    if (got == want) {
        n_passed++;
        printf("npipe_test: %s: ok\n", name);
    } else {
        printf("npipe_test: %s: FAIL got %lu want %lu\n", name, got, want);
    }
    fflush(stdout);
}

static void check_str(const char *name, const char *got, const char *want)
{
    n_checks++;
    if (strcmp(got, want) == 0) {
        n_passed++;
        printf("npipe_test: %s: ok\n", name);
    } else {
        printf("npipe_test: %s: FAIL got '%s' want '%s'\n", name, got, want);
    }
    fflush(stdout);
}

/* The program's full name: the string right before the command line. */
static const char *own_name(void)
{
    PTIB ptib;
    PPIB ppib;
    const char *p;
    DosGetInfoBlocks(&ptib, &ppib);
    p = ppib->pib_pchcmd - 1;
    while (p[-1] != '\0')
        p--;
    return p;
}

static void no_inherit(HFILE h)
{
    ULONG state = 0;
    DosQueryFHState(h, &state);
    state &= 0x7F88;
    DosSetFHState(h, state | OPEN_FLAGS_NOINHERIT);
}

/* Start "child <args>" (a copy of this program). */
static APIRET start_child(ULONG flags, const char *args, RESULTCODES *res)
{
    char argbuf[256];
    char fail[64];
    const char *me = own_name();
    size_t n = strlen(me);
    memcpy(argbuf, me, n + 1);
    sprintf(argbuf + n + 1, "child %s", args);
    argbuf[n + 1 + strlen(argbuf + n + 1) + 1] = '\0';
    /* An environment of its own: the child refuses to act as the parent
     * (no endless chain of copies if its arguments were lost). */
    return DosExecPgm(fail, sizeof(fail), flags, (PSZ)argbuf, (PSZ)"NPIPE_TEST_CHILD=1\0", res, (PSZ)me);
}

static ULONG wait_child(PID pid)
{
    RESULTCODES res;
    PID done;
    if (DosWaitChild(DCWA_PROCESS, DCWW_WAIT, &res, &done, pid) != NO_ERROR)
        return 0xFFFF;
    return res.codeResult;
}

/* ── Child side ─────────────────────────────────────────────────────── */

static int child_named(void)
{
    HPIPE h;
    ULONG action, n, failures = 0;
    char buf[64];
    if (DosOpen((PSZ)XPROC_PIPE, &h, &action, 0, 0, OPEN_ACTION_OPEN_IF_EXISTS,
                OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL) != NO_ERROR)
        return 100;
    if (DosWrite(h, "ping from child", 15, &n) != NO_ERROR) failures++;
    if (DosSetNPHState(h, NP_WAIT | NP_READMODE_MESSAGE) != NO_ERROR) failures++;
    memset(buf, 0, sizeof(buf));
    if (DosRead(h, buf, sizeof(buf) - 1, &n) != NO_ERROR || strcmp(buf, "pong") != 0) failures++;
    DosClose(h);
    /* Second exchange: wait for the instance to listen again, then call. */
    if (DosWaitNPipe((PSZ)XPROC_PIPE, NP_INDEFINITE_WAIT) != NO_ERROR) failures++;
    memset(buf, 0, sizeof(buf));
    if (DosCallNPipe((PSZ)XPROC_PIPE, "call", 4, buf, sizeof(buf) - 1, &n, 5000) != NO_ERROR
        || strcmp(buf, "re:call") != 0) failures++;
    return (int)failures;
}

static int child_main(int argc, char **argv)
{
    ULONG n, total = 0, type = 0, attr;
    char buf[256];
    if (argc >= 4 && strcmp(argv[2], "anon") == 0) {
        HFILE h = (HFILE)strtoul(argv[3], NULL, 10);
        return DosWrite(h, "anon-from-child", 15, &n) == NO_ERROR ? 0 : 1;
    }
    if (argc >= 3 && strcmp(argv[2], "stdin") == 0) {
        DosQueryHType(0, &type, &attr);
        if ((type & 0xFF) != HANDTYPE_PIPE)
            return 200;
        while (DosRead(0, buf, sizeof(buf), &n) == NO_ERROR && n > 0)
            total += n;
        return (int)total;
    }
    if (argc >= 3 && strcmp(argv[2], "stdout") == 0) {
        return DosWrite(1, "child-stdout", 12, &n) == NO_ERROR ? 0 : 1;
    }
    if (argc >= 3 && strcmp(argv[2], "named") == 0)
        return child_named();
    if (argc >= 3 && strcmp(argv[2], "env") == 0) {
        PCSZ v = NULL;
        return DosScanEnv((PCSZ)"NPIPE_TEST_CHILD", (PSZ *)&v) == NO_ERROR && strcmp(v, "1") == 0 ? 0 : 1;
    }
    return 99;
}

/* Copied as FIND.EXE: print the lines of standard input that contain the
 * argument (quotes removed), as a pipeline's second program. */
static int find_main(int argc, char **argv)
{
    char pat[128], line[512], c;
    ULONG n, len = 0, found = 0;
    char *p = argc >= 2 ? argv[1] : "";
    size_t k = 0;
    for (; *p && k < sizeof(pat) - 1; p++)
        if (*p != '"')
            pat[k++] = *p;
    pat[k] = '\0';
    for (;;) {
        APIRET rc = DosRead(0, &c, 1, &n);
        if (rc != NO_ERROR || n == 0 || c == '\n' || len == sizeof(line) - 1) {
            line[len] = '\0';
            if (len > 0 && line[len - 1] == '\r')
                line[--len] = '\0';
            if (len > 0 && strstr(line, pat) != NULL) {
                found++;
                DosWrite(1, line, len, &n);
                DosWrite(1, "\r\n", 2, &n);
            }
            len = 0;
            if (rc != NO_ERROR || n == 0)
                break;
        } else {
            line[len++] = c;
        }
    }
    return found ? 0 : 1;
}

/* Whether the program runs under the name `name` (FIND.EXE). */
static int named_as(const char *name)
{
    const char *me = own_name();
    size_t a = strlen(me), b = strlen(name);
    return a >= b && stricmp(me + a - b, name) == 0;
}

/* ── Parent side ────────────────────────────────────────────────────── */

/* An anonymous pipe handle passed by number to the child. */
static void test_anon_handle(void)
{
    HFILE r, w;
    RESULTCODES res;
    char args[32], buf[64];
    ULONG n, got = 0;
    check("DosCreatePipe", DosCreatePipe(&r, &w, 4096), NO_ERROR);
    no_inherit(r);
    sprintf(args, "anon %lu", (ULONG)w);
    check("DosExecPgm(anon)", start_child(EXEC_ASYNCRESULT, args, &res), NO_ERROR);
    DosClose(w);
    memset(buf, 0, sizeof(buf));
    while (got < sizeof(buf) - 1 && DosRead(r, buf + got, sizeof(buf) - 1 - got, &n) == NO_ERROR && n > 0)
        got += n;
    check_str("child wrote to the inherited handle", buf, "anon-from-child");
    check("anon child exit code", wait_child(res.codeTerminate), 0);
    DosClose(r);
}

/* A pipe as the child's standard input; EOF once the parent closes. */
static void test_child_stdin(void)
{
    HFILE r, w, saved = (HFILE)-1, zero = 0;
    RESULTCODES res;
    ULONG n;
    DosCreatePipe(&r, &w, 4096);
    no_inherit(w);
    DosDupHandle(0, &saved);
    DosDupHandle(r, &zero);
    check("DosExecPgm(stdin)", start_child(EXEC_ASYNCRESULT, "stdin", &res), NO_ERROR);
    zero = 0;
    DosDupHandle(saved, &zero);
    DosClose(saved);
    DosClose(r);
    DosWrite(w, "hello child\r\n", 13, &n);
    DosClose(w);
    check("child read its piped stdin to the end", wait_child(res.codeTerminate), 13);
}

/* A pipe as the child's standard output. */
/* The environment given to DosExecPgm reaches the child. */
static void test_child_env(void)
{
    RESULTCODES res;
    check("DosExecPgm(env, sync)", start_child(EXEC_SYNC, "env", &res), NO_ERROR);
    check("child saw its environment", res.codeResult, 0);
}

static void test_child_stdout(void)
{
    HFILE r, w, saved = (HFILE)-1, one = 1;
    RESULTCODES res;
    char buf[64];
    ULONG n, got = 0;
    APIRET rc;
    DosCreatePipe(&r, &w, 4096);
    no_inherit(r);
    DosDupHandle(1, &saved);
    DosDupHandle(w, &one);
    rc = start_child(EXEC_SYNC, "stdout", &res);
    one = 1;
    DosDupHandle(saved, &one);
    /* Reported once standard output is the console again. */
    check("DosExecPgm(stdout, sync)", rc, NO_ERROR);
    DosClose(saved);
    DosClose(w);
    memset(buf, 0, sizeof(buf));
    while (got < sizeof(buf) - 1 && DosRead(r, buf + got, sizeof(buf) - 1 - got, &n) == NO_ERROR && n > 0)
        got += n;
    DosClose(r);
    check_str("child wrote to its piped stdout", buf, "child-stdout");
}

/* A named pipe served here, used by a child process. */
static void test_named_across_processes(void)
{
    HPIPE h;
    RESULTCODES res;
    char buf[64];
    ULONG n, rc;
    rc = DosCreateNPipe((PSZ)XPROC_PIPE, &h, NP_ACCESS_DUPLEX,
                        NP_WAIT | NP_TYPE_MESSAGE | NP_READMODE_MESSAGE | 1, 512, 512, 0);
    check("DosCreateNPipe(xproc)", rc, NO_ERROR);
    check("DosExecPgm(named)", start_child(EXEC_ASYNCRESULT, "named", &res), NO_ERROR);
    check("DosConnectNPipe waits for the child", DosConnectNPipe(h), NO_ERROR);
    memset(buf, 0, sizeof(buf));
    check("DosRead(message from child)", DosRead(h, buf, sizeof(buf) - 1, &n), NO_ERROR);
    check_str("child's message", buf, "ping from child");
    check("DosWrite(reply)", DosWrite(h, "pong", 4, &n), NO_ERROR);
    /* The child closes; acknowledge and listen again for DosCallNPipe. */
    memset(buf, 0, sizeof(buf));
    check("DosRead at the client's close", DosRead(h, buf, sizeof(buf) - 1, &n), NO_ERROR);
    check("  no data", n, 0);
    check("DosDisConnectNPipe", DosDisConnectNPipe(h), NO_ERROR);
    check("DosConnectNPipe again", DosConnectNPipe(h), NO_ERROR);
    memset(buf, 0, sizeof(buf));
    DosRead(h, buf, sizeof(buf) - 1, &n);
    check_str("DosCallNPipe request", buf, "call");
    DosWrite(h, "re:call", 7, &n);
    check("named child failures", wait_child(res.codeTerminate), 0);
    DosClose(h);
}

/* Server and client in this process. */
static void test_named_local(void)
{
    HPIPE hs, hc;
    ULONG action, n, state;
    AVAILDATA avail;
    char buf[64];
    BYTE info[64];
    PIPEINFO *pi = (PIPEINFO *)info;

    check("DosCreateNPipe(bad name)", DosCreateNPipe((PSZ)"\\NOPIPE\\X", &hs, NP_ACCESS_DUPLEX,
          NP_WAIT | 1, 64, 64, 0), ERROR_PATH_NOT_FOUND);
    check("DosCreateNPipe(bad mode)", DosCreateNPipe((PSZ)LOCAL_PIPE, &hs, NP_ACCESS_DUPLEX,
          NP_READMODE_MESSAGE | 1, 64, 64, 0), ERROR_INVALID_PARAMETER);
    check("DosCreateNPipe(local)", DosCreateNPipe((PSZ)LOCAL_PIPE, &hs, NP_ACCESS_DUPLEX,
          NP_NOWAIT | NP_TYPE_MESSAGE | NP_READMODE_MESSAGE | 1, 1024, 2048, 0), NO_ERROR);
    check("second instance over the limit", DosCreateNPipe((PSZ)LOCAL_PIPE, &hc, NP_ACCESS_DUPLEX,
          NP_TYPE_MESSAGE | 1, 64, 64, 0), ERROR_PIPE_BUSY);
    check("DosOpen before listening", DosOpen((PSZ)LOCAL_PIPE, &hc, &action, 0, 0, OPEN_ACTION_OPEN_IF_EXISTS,
          OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL), ERROR_PIPE_BUSY);
    check("DosConnectNPipe(non-blocking)", DosConnectNPipe(hs), ERROR_PIPE_NOT_CONNECTED);
    DosPeekNPipe(hs, buf, sizeof(buf), &n, &avail, &state);
    check("state listening", state, NP_STATE_LISTENING);
    check("DosOpen(client)", DosOpen((PSZ)LOCAL_PIPE, &hc, &action, 0, 0, OPEN_ACTION_OPEN_IF_EXISTS,
          OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL), NO_ERROR);
    while (DosConnectNPipe(hs) == ERROR_PIPE_NOT_CONNECTED)
        DosSleep(1);
    DosQueryNPHState(hs, &state);
    check("server handle state", state, NP_NOWAIT | NP_END_SERVER | NP_TYPE_MESSAGE | NP_READMODE_MESSAGE | 1);
    DosQueryNPHState(hc, &state);
    check("client handle state", state, NP_TYPE_MESSAGE | 1);
    DosWrite(hc, "first", 5, &n);
    DosWrite(hc, "second", 6, &n);
    DosSleep(20);
    memset(buf, 0, sizeof(buf));
    check("DosPeekNPipe", DosPeekNPipe(hs, buf, 3, &n, &avail, &state), NO_ERROR);
    check("  peeked bytes", n, 3);
    check("  bytes in the pipe", avail.cbpipe, 11 + 2 * 2);
    check("  current message", avail.cbmessage, 5);
    check("  state connected", state, NP_STATE_CONNECTED);
    memset(buf, 0, sizeof(buf));
    check("DosRead(short buffer)", DosRead(hs, buf, 3, &n), ERROR_MORE_DATA);
    check_str("  first part", buf, "fir");
    memset(buf, 0, sizeof(buf));
    DosRead(hs, buf, sizeof(buf), &n);
    check_str("  rest of the message", buf, "st");
    memset(buf, 0, sizeof(buf));
    DosRead(hs, buf, sizeof(buf), &n);
    check_str("  next message", buf, "second");
    check("DosRead(empty, non-blocking)", DosRead(hs, buf, sizeof(buf), &n), ERROR_NO_DATA);
    check("DosSetNPHState(reserved bit)", DosSetNPHState(hs, 1), ERROR_INVALID_PARAMETER);
    check("DosQueryNPipeInfo(1)", DosQueryNPipeInfo(hs, 1, info, sizeof(info)), NO_ERROR);
    check("  cbOut", pi->cbOut, 1024);
    check("  cbIn", pi->cbIn, 2048);
    check("  cbMaxInst", pi->cbMaxInst, 1);
    check_str("  szName", pi->szName, LOCAL_PIPE);
    check("DosQueryNPipeInfo(short)", DosQueryNPipeInfo(hs, 1, info, 4), ERROR_BUFFER_OVERFLOW);
    check("DosQueryNPipeInfo(level 3)", DosQueryNPipeInfo(hs, 3, info, sizeof(info)), ERROR_INVALID_LEVEL);
    check("DosTransactNPipe(byte-read client)", DosTransactNPipe(hc, "x", 1, buf, sizeof(buf), &n),
          ERROR_BAD_FORMAT);
    check("DosWaitNPipe(unknown)", DosWaitNPipe((PSZ)"\\PIPE\\NPTEST\\NONE", 10), ERROR_FILE_NOT_FOUND);
    check("DosConnectNPipe(client handle)", DosConnectNPipe(hc), ERROR_BAD_PIPE);
    DosWrite(hs, "lost", 4, &n);
    check("DosDisConnectNPipe", DosDisConnectNPipe(hs), NO_ERROR);
    check("client read after disconnect", DosRead(hc, buf, sizeof(buf), &n), ERROR_PIPE_NOT_CONNECTED);
    check("client write after disconnect", DosWrite(hc, "x", 1, &n), ERROR_BROKEN_PIPE);
    DosPeekNPipe(hs, buf, sizeof(buf), &n, &avail, &state);
    check("state disconnected", state, NP_STATE_DISCONNECTED);
    DosClose(hc);
    DosClose(hs);
}

static void test_give_shared_mem(void)
{
    PVOID named, giveable, plain, again;
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks(&ptib, &ppib);
    check("DosAllocSharedMem(named)", DosAllocSharedMem(&named, (PSZ)SHMEM_NAME, 4096,
          PAG_COMMIT | PAG_READ | PAG_WRITE), NO_ERROR);
    check("same name again", DosAllocSharedMem(&again, (PSZ)SHMEM_NAME, 4096,
          PAG_COMMIT | PAG_READ | PAG_WRITE), ERROR_ALREADY_EXISTS);
    check("DosAllocSharedMem(giveable)", DosAllocSharedMem(&giveable, NULL, 4096,
          PAG_COMMIT | PAG_READ | PAG_WRITE | OBJ_GIVEABLE), NO_ERROR);
    check("DosAllocSharedMem(not giveable)", DosAllocSharedMem(&plain, NULL, 4096,
          PAG_COMMIT | PAG_READ | OBJ_GETTABLE), NO_ERROR);
    check("DosGiveSharedMem(no access flag)", DosGiveSharedMem(giveable, ppib->pib_ulpid, PAG_COMMIT),
          ERROR_INVALID_PARAMETER);
    check("DosGiveSharedMem(not a block)", DosGiveSharedMem((PBYTE)giveable + 16, ppib->pib_ulpid, PAG_READ),
          ERROR_INVALID_ADDRESS);
    check("DosGiveSharedMem(not giveable)", DosGiveSharedMem(plain, ppib->pib_ulpid, PAG_READ),
          ERROR_ACCESS_DENIED);
    check("DosGiveSharedMem(more than allowed)", DosGiveSharedMem(plain, ppib->pib_ulpid, PAG_EXECUTE),
          ERROR_ACCESS_DENIED);
    check("DosGiveSharedMem(unknown process)", DosGiveSharedMem(giveable, 0x7777, PAG_READ),
          ERROR_INVALID_PROCID);
    check("DosFreeMem(named)", DosFreeMem(named), NO_ERROR);
    check("name free again", DosAllocSharedMem(&again, (PSZ)SHMEM_NAME, 4096,
          PAG_COMMIT | PAG_READ | PAG_WRITE), NO_ERROR);
    DosFreeMem(again);
    DosFreeMem(giveable);
    DosFreeMem(plain);
}

/* The arguments, split at blanks, from the command line in the PIB (the
 * second string at pib_pchcmd); argv[0] is the program name. */
static int pib_args(char **argv, int max)
{
    static char line[256];
    PTIB ptib;
    PPIB ppib;
    char *p;
    int argc = 1;
    DosGetInfoBlocks(&ptib, &ppib);
    argv[0] = ppib->pib_pchcmd;
    strncpy(line, ppib->pib_pchcmd + strlen(ppib->pib_pchcmd) + 1, sizeof(line) - 1);
    for (p = strtok(line, " "); p != NULL && argc < max; p = strtok(NULL, " "))
        argv[argc++] = p;
    return argc;
}

int main(void)
{
    char *argv[8];
    int argc = pib_args(argv, 8);
    PCSZ child_env = NULL;
    if (named_as("FIND.EXE"))
        return find_main(argc, argv);
    if (argc >= 2 && strcmp(argv[1], "child") == 0)
        return child_main(argc, argv);
    if (DosScanEnv((PCSZ)"NPIPE_TEST_CHILD", (PSZ *)&child_env) == NO_ERROR)
        return 98;
    test_named_local();
    test_give_shared_mem();
    test_anon_handle();
    test_child_stdin();
    test_child_stdout();
    test_child_env();
    test_named_across_processes();
    printf("npipe_test: done %d/%d\n", n_passed, n_checks);
    return n_passed == n_checks ? 0 : 1;
}
