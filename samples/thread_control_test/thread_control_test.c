/*
 * thread_control_test.c - stopping threads that spin in guest code;
 * DosSuspendThread / DosResumeThread; signal exceptions
 *
 * Checks:
 *   - a thread spinning in a pure loop (no system calls) is stopped at
 *     once by DosSuspendThread, by a critical section and by
 *     DosKillThread; suspensions count; a killed suspended thread ends
 *     when resumed; DosCreateThread(CREATE_SUSPENDED) waits for a resume;
 *     the error codes of DosSuspendThread / DosResumeThread;
 *   - DosSetSignalExceptionFocus counts; DosSendSignalException to the
 *     process itself reaches thread 1's exception handler as XCPT_SIGNAL,
 *     also while thread 1 spins in a pure loop; argument errors of
 *     DosSendSignalException / DosAcknowledgeSignalException;
 *   - with the program copied to C:\THRDCF.EXE and C:\THRDCN.EXE (the
 *     integration test does this), a child process started from the first
 *     asks for signals and gets XCPT_SIGNAL_INTR from DosSendSignalException
 *     (exit code 7); one started from the second has not asked for them
 *     and is ended by it (exit code 1). The copies tell by their module
 *     name which child they are (Warpine passes no command-line arguments
 *     to a program yet).
 *
 * Each check is written to stdout:
 *
 *   thread_control_test: <name>: ok
 *   thread_control_test: <name>: FAIL got <value> want <value>
 *
 * followed by "thread_control_test: done <passed>/<total>".
 */
#define INCL_DOSPROCESS
#define INCL_DOSEXCEPTIONS
#define INCL_DOSFILEMGR
#define INCL_DOSMISC
#define INCL_DOSMODULEMGR
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define STACK_SIZE      0x4000
#define CHILD_FOCUS     "C:\\THRDCF.EXE"
#define CHILD_NOFOCUS   "C:\\THRDCN.EXE"
#define READY_FILE      "C:\\THRDCTL.RDY"
#define CHILD_GOT_INTR  7
#define CHILD_NO_SIGNAL 8
/* Spinning: SPIN_INNER increments between looks at the clock. */
#define SPIN_INNER      10000000UL
#define SPIN_LIMIT_MS   20000UL

static int n_checks = 0;
static int n_passed = 0;

static volatile ULONG spin_count;
static volatile ULONG started_flag;
static volatile ULONG got_signal;
static volatile ULONG signal_count;
static volatile ULONG sink;
static ULONG          own_pid;

/* ---------- output (no C library stdio: not every thread has one) ---------- */

static char *append_str( char *p, const char *s )
{
    while( *s ) *p++ = *s++;
    *p = '\0';
    return p;
}

static char *append_long( char *p, LONG n )
{
    char  tmp[12];
    int   i = 0;
    ULONG u = n < 0 ? (ULONG)-n : (ULONG)n;
    if( n < 0 ) *p++ = '-';
    do { tmp[i++] = (char)( '0' + u % 10 ); u /= 10; } while( u );
    while( i ) *p++ = tmp[--i];
    *p = '\0';
    return p;
}

static void write_stdout( const char *s )
{
    ULONG written;
    DosWrite( 1, (PVOID)s, strlen( s ), &written );
}

static void check( const char *what, LONG got, LONG want )
{
    char  line[160];
    char *p = append_str( line, "thread_control_test: " );
    p = append_str( p, what );
    if( got == want ) {
        append_str( p, ": ok\r\n" );
        n_passed++;
    } else {
        p = append_str( p, ": FAIL got " );
        p = append_long( p, got );
        p = append_str( p, " want " );
        p = append_long( p, want );
        append_str( p, "\r\n" );
    }
    write_stdout( line );
    n_checks++;
}

static void done( void )
{
    char  line[64];
    char *p = append_str( line, "thread_control_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );
}

/* ---------- threads ---------- */

/* Spins without ever calling the system. */
static VOID APIENTRY spinner( ULONG arg )
{
    (void)arg;
    for( ;; ) spin_count++;
}

static VOID APIENTRY starter( ULONG arg )
{
    started_flag = arg;
}

static VOID APIENTRY break_sender( ULONG arg )
{
    (void)arg;
    DosSleep( 100 );
    DosSendSignalException( own_pid, XCPT_SIGNAL_BREAK );
}

/* Did the spinner advance over `ms` milliseconds? */
static ULONG spins_during( ULONG ms )
{
    ULONG before = spin_count;
    DosSleep( ms );
    return spin_count != before;
}

/* ---------- signal handler of thread 1 ---------- */

static ULONG APIENTRY signal_handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                                      PCONTEXTRECORD ctx, PVOID unused )
{
    (void)reg; (void)ctx; (void)unused;
    if( rep->ExceptionNum == XCPT_SIGNAL ) {
        got_signal = rep->ExceptionInfo[0];
        signal_count++;
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

/* Spin until a signal arrived or SPIN_LIMIT_MS passed: SPIN_INNER
   increments without system calls between looks at the clock. */
static void spin_until_signal( void )
{
    ULONG start, now, i;
    DosQuerySysInfo( QSV_MS_COUNT, QSV_MS_COUNT, &start, sizeof( start ) );
    while( !got_signal ) {
        for( i = 0; i < SPIN_INNER && !got_signal; i++ ) sink++;
        DosQuerySysInfo( QSV_MS_COUNT, QSV_MS_COUNT, &now, sizeof( now ) );
        if( now - start > SPIN_LIMIT_MS ) break;
    }
}

/* ---------- the child processes ---------- */

static void make_ready_file( void )
{
    HFILE hf;
    ULONG action;
    if( DosOpen( READY_FILE, &hf, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                 OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYNONE, NULL ) == NO_ERROR ) {
        DosClose( hf );
    }
}

static int child( int with_focus )
{
    EXCEPTIONREGISTRATIONRECORD rec;
    ULONG times;
    rec.prev_structure = NULL;
    rec.ExceptionHandler = signal_handler;
    DosSetExceptionHandler( &rec );
    if( with_focus ) DosSetSignalExceptionFocus( SIG_SETFOCUS, &times );
    make_ready_file();
    spin_until_signal();
    DosUnsetExceptionHandler( &rec );
    return got_signal == XCPT_SIGNAL_INTR ? CHILD_GOT_INTR : CHILD_NO_SIGNAL;
}

/* Start child program `exe`, signal it once it is ready; its result code. */
static LONG run_signalled_child( const char *exe, LONG *send_rc )
{
    char        args[64];
    char       *p;
    RESULTCODES res;
    PID         pid, ended;
    FILESTATUS3 fs;
    int         tries;
    APIRET      rc;

    DosDelete( READY_FILE );
    p = append_str( args, exe ) + 1;
    *p = '\0';
    rc = DosExecPgm( NULL, 0, EXEC_ASYNC, args, NULL, &res, exe );
    if( rc != NO_ERROR ) return -(LONG)rc;
    /* EXEC_ASYNC: the child's process id is in codeTerminate (CP Reference). */
    pid = res.codeTerminate;
    for( tries = 0; tries < 200; tries++ ) {
        if( DosQueryPathInfo( READY_FILE, FIL_STANDARD, &fs, sizeof( fs ) ) == NO_ERROR ) break;
        DosSleep( 50 );
    }
    DosSleep( 50 );
    *send_rc = DosSendSignalException( pid, XCPT_SIGNAL_INTR );
    rc = DosWaitChild( DCWA_PROCESS, DCWW_WAIT, &res, &ended, pid );
    DosDelete( READY_FILE );
    if( rc != NO_ERROR ) return -(LONG)rc;
    return res.codeResult;
}

/* ---------- main ---------- */

/* Whether this program runs as module `name`. */
static int named( PPIB ppib, const char *name )
{
    char module[260];
    if( DosQueryModuleName( ppib->pib_hmte, sizeof( module ), module ) != NO_ERROR ) return 0;
    return stricmp( module, name ) == 0;
}

int main( void )
{
    TID   tid, tid2;
    ULONG rc, times;
    PTIB  ptib;
    PPIB  ppib;
    LONG  result, send_rc;
    FILESTATUS3 fs;
    EXCEPTIONREGISTRATIONRECORD rec;

    DosGetInfoBlocks( &ptib, &ppib );
    if( named( ppib, CHILD_FOCUS ) ) return child( 1 );
    if( named( ppib, CHILD_NOFOCUS ) ) return child( 0 );

    own_pid = ppib->pib_ulpid;

    /* ---- DosSuspendThread / DosResumeThread on a spinning thread ---- */
    rc = DosCreateThread( &tid, spinner, 0, CREATE_READY | STACK_SPARSE, STACK_SIZE );
    check( "DosCreateThread(spinner)", rc, NO_ERROR );
    DosSleep( 50 );
    check( "the spinner spins", spins_during( 100 ), 1 );
    check( "DosSuspendThread(spinner)", DosSuspendThread( tid ), NO_ERROR );
    DosSleep( 20 );
    check( "-> it stops at once", spins_during( 200 ), 0 );
    check( "DosSuspendThread(spinner) again", DosSuspendThread( tid ), NO_ERROR );
    check( "DosResumeThread once", DosResumeThread( tid ), NO_ERROR );
    check( "-> still suspended (count 1)", spins_during( 150 ), 0 );
    check( "DosResumeThread twice", DosResumeThread( tid ), NO_ERROR );
    check( "-> it spins again", spins_during( 150 ), 1 );
    check( "DosResumeThread(running thread)", DosResumeThread( tid ), ERROR_NOT_FROZEN );
    check( "DosSuspendThread(itself)", DosSuspendThread( 1 ), ERROR_INVALID_THREADID );
    check( "DosSuspendThread(no such thread)", DosSuspendThread( 999 ), ERROR_INVALID_THREADID );
    check( "DosResumeThread(no such thread)", DosResumeThread( 999 ), ERROR_INVALID_THREADID );

    /* ---- a critical section stops it ---- */
    check( "DosEnterCritSec", DosEnterCritSec(), NO_ERROR );
    DosSleep( 20 );
    check( "-> the spinner stops inside the section", spins_during( 200 ), 0 );
    check( "DosExitCritSec", DosExitCritSec(), NO_ERROR );
    check( "-> it spins after the section", spins_during( 150 ), 1 );

    /* ---- DosKillThread ends it ---- */
    check( "DosKillThread(spinner)", DosKillThread( tid ), NO_ERROR );
    tid2 = tid;
    check( "-> DosWaitThread returns", DosWaitThread( &tid2, DCWW_WAIT ), NO_ERROR );
    check( "-> it no longer spins", spins_during( 100 ), 0 );
    check( "DosSuspendThread(ended thread)", DosSuspendThread( tid ), ERROR_INVALID_THREADID );

    /* ---- a killed suspended thread ends when resumed ---- */
    rc = DosCreateThread( &tid, spinner, 0, 0, STACK_SIZE );
    check( "DosCreateThread(second spinner)", rc, NO_ERROR );
    DosSleep( 50 );
    check( "DosSuspendThread(second spinner)", DosSuspendThread( tid ), NO_ERROR );
    check( "DosKillThread(suspended spinner)", DosKillThread( tid ), NO_ERROR );
    DosSleep( 100 );
    tid2 = tid;
    check( "-> not ended while suspended", DosWaitThread( &tid2, DCWW_NOWAIT ),
           ERROR_THREAD_NOT_TERMINATED );
    check( "DosResumeThread(killed spinner)", DosResumeThread( tid ), NO_ERROR );
    tid2 = tid;
    check( "-> it ends", DosWaitThread( &tid2, DCWW_WAIT ), NO_ERROR );

    /* ---- CREATE_SUSPENDED ---- */
    rc = DosCreateThread( &tid, starter, 5, CREATE_SUSPENDED | STACK_COMMITTED, STACK_SIZE );
    check( "DosCreateThread(CREATE_SUSPENDED)", rc, NO_ERROR );
    DosSleep( 100 );
    check( "-> it has not run", started_flag, 0 );
    check( "DosResumeThread(created suspended)", DosResumeThread( tid ), NO_ERROR );
    tid2 = tid;
    DosWaitThread( &tid2, DCWW_WAIT );
    check( "-> it ran", started_flag, 5 );

    /* ---- signal exceptions within the process ---- */
    rec.prev_structure = NULL;
    rec.ExceptionHandler = signal_handler;
    check( "DosSetExceptionHandler", DosSetExceptionHandler( &rec ), NO_ERROR );
    times = 99;
    check( "DosSetSignalExceptionFocus(SIG_SETFOCUS)",
           DosSetSignalExceptionFocus( SIG_SETFOCUS, &times ), NO_ERROR );
    check( "-> times", times, 1 );
    check( "DosSendSignalException(self, INTR)",
           DosSendSignalException( own_pid, XCPT_SIGNAL_INTR ), NO_ERROR );
    check( "-> thread 1's handler got XCPT_SIGNAL_INTR", got_signal, XCPT_SIGNAL_INTR );
    got_signal = 0;
    rc = DosCreateThread( &tid, break_sender, 0, 0, STACK_SIZE );
    check( "DosCreateThread(break sender)", rc, NO_ERROR );
    spin_until_signal();
    check( "-> XCPT_SIGNAL_BREAK reaches thread 1 spinning in a pure loop", got_signal,
           XCPT_SIGNAL_BREAK );
    tid2 = tid;
    DosWaitThread( &tid2, DCWW_WAIT );
    check( "-> handled twice", signal_count, 2 );
    check( "DosSendSignalException(KILLPROC)",
           DosSendSignalException( own_pid, XCPT_SIGNAL_KILLPROC ), ERROR_INVALID_SIGNAL_NUMBER );
    check( "DosSendSignalException(signal 0)",
           DosSendSignalException( own_pid, 0 ), ERROR_INVALID_SIGNAL_NUMBER );
    check( "DosSendSignalException(no such process)",
           DosSendSignalException( 12345, XCPT_SIGNAL_INTR ), ERROR_INVALID_PROCID );
    check( "DosAcknowledgeSignalException(INTR)",
           DosAcknowledgeSignalException( XCPT_SIGNAL_INTR ), NO_ERROR );
    check( "DosAcknowledgeSignalException(2)",
           DosAcknowledgeSignalException( 2 ), ERROR_INVALID_SIGNAL_NUMBER );
    check( "DosSetSignalExceptionFocus(SIG_UNSETFOCUS)",
           DosSetSignalExceptionFocus( SIG_UNSETFOCUS, &times ), NO_ERROR );
    check( "-> times", times, 0 );
    check( "DosSetSignalExceptionFocus(SIG_UNSETFOCUS) at 0",
           DosSetSignalExceptionFocus( SIG_UNSETFOCUS, &times ), ERROR_ALREADY_RESET );
    DosUnsetExceptionHandler( &rec );

    /* ---- signal exceptions to a child process ---- */
    if( DosQueryPathInfo( CHILD_FOCUS, FIL_STANDARD, &fs, sizeof( fs ) ) == NO_ERROR
        && DosQueryPathInfo( CHILD_NOFOCUS, FIL_STANDARD, &fs, sizeof( fs ) ) == NO_ERROR ) {
        send_rc = -1;
        result = run_signalled_child( CHILD_FOCUS, &send_rc );
        check( "DosSendSignalException(child, INTR)", send_rc, NO_ERROR );
        check( "-> the child's handler got it", result, CHILD_GOT_INTR );
        send_rc = -1;
        result = run_signalled_child( CHILD_NOFOCUS, &send_rc );
        check( "DosSendSignalException(child without focus, INTR)", send_rc, NO_ERROR );
        check( "-> it ends the child", result, 1 );
    } else {
        write_stdout( "thread_control_test: (child checks skipped: no " CHILD_FOCUS
                      " / " CHILD_NOFOCUS ")\r\n" );
    }

    done();
    return 0;
}
