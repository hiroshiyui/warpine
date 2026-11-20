/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * kill_test.c - how processes end: DosKillProcess, exit lists, blocking
 * calls interrupted by signals, Ctrl+C from the keyboard
 *
 * The program starts copies of itself; a copy finds its mode in its
 * command line ("child <mode>", the second string at pib_pchcmd), never
 * in argv. Modes (C:\KT_<MODE>.RDY says a child is ready, C:\KT_<MODE>.OUT
 * holds what it reports):
 *
 *   exitlist  sleeps in DosSleep with an exit-list routine; killed with
 *             DKP_PROCESS: the routine runs with TC_KILLPROCESS, the
 *             parent's DosWaitChild reports TC_KILLPROCESS
 *   handler   a handler continues XCPT_SIGNAL_KILLPROC: its DosSleep fails
 *             with ERROR_INTERRUPT and it ends by itself (code 9)
 *   ignore    a handler continues XCPT_SIGNAL_KILLPROC and the child sleeps
 *             on: it is ended after the grace period (TC_KILLPROCESS)
 *   tree      starts "grand" and waits for it; killed with DKP_PROCESSTREE,
 *             both end and the grandchild's exit list runs
 *   kbd       (started by the integration test with Ctrl+C and "x" as
 *             piped keyboard input) KbdCharIn returns "x", the handler got
 *             XCPT_SIGNAL_INTR
 *   eof       (integration test, empty piped input) an exit-list routine
 *             runs with TC_EXIT when KbdCharIn finds the end of input
 *   win       (integration test, VIO window under a GUI script) records
 *             XCPT_SIGNAL_INTR from Ctrl+C and the exit list's termination
 *             code when the window is closed, in C:\KT_WIN.OUT
 *
 * Without a mode the program also checks, on thread 1: DosSleep,
 * DosWaitEventSem and DosRead of a pipe interrupted by a signal another
 * thread sends; DosKillProcess error codes.
 *
 * Each check is written to stdout:
 *
 *   kill_test: <name>: ok
 *   kill_test: <name>: FAIL got <value> want <value>
 *
 * followed by "kill_test: done <passed>/<total>".
 */
#define INCL_DOS
#define INCL_DOSEXCEPTIONS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define STACK_SIZE 0x4000
#define KILL_GRACE_MAX_MS 15000UL

/* KbdCharIn, KBDCALLS 4, as a direct 32-bit import (_Pascal: arguments
   pushed left to right, removed by the callee), as 4OS2's headers do. */
typedef struct {
    UCHAR  chChar;
    UCHAR  chScan;
    UCHAR  fbStatus;
    UCHAR  bNlsShift;
    USHORT fsState;
    ULONG  time;
} KEYINFO;
extern USHORT _Pascal KBD16CHARIN( KEYINFO *info, USHORT wait, USHORT hkbd );

static int n_checks = 0;
static int n_passed = 0;
static volatile ULONG got_signal;
static volatile ULONG signal_count;
static ULONG own_pid;
static char  out_name[32];

/* ---------- output ---------- */

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
    char  line[200];
    char *p = append_str( line, "kill_test: " );
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

static ULONG now_ms( void )
{
    ULONG ms;
    DosQuerySysInfo( QSV_MS_COUNT, QSV_MS_COUNT, &ms, sizeof( ms ) );
    return ms;
}

/* ---------- files on drive C ---------- */

static void file_name( char *buf, const char *mode, const char *ext )
{
    char *p = append_str( buf, "C:\\KT_" );
    while( *mode ) {
        char c = *mode++;
        *p++ = (char)( c >= 'a' && c <= 'z' ? c - 'a' + 'A' : c );
    }
    p = append_str( p, "." );
    append_str( p, ext );
}

/* Append `text` to file `name` (created when missing). */
static void append_file( const char *name, const char *text )
{
    HFILE hf;
    ULONG action, written, pos;
    if( DosOpen( (PSZ)name, &hf, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_OPEN_IF_EXISTS,
                 OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYNONE, NULL ) != NO_ERROR ) return;
    DosSetFilePtr( hf, 0, FILE_END, &pos );
    DosWrite( hf, (PVOID)text, strlen( text ), &written );
    DosClose( hf );
}

/* Read file `name` into `buf` (NUL-terminated); its length, -1 if none. */
static LONG read_file( const char *name, char *buf, ULONG size )
{
    HFILE hf;
    ULONG action, got = 0;
    buf[0] = '\0';
    if( DosOpen( (PSZ)name, &hf, &action, 0, FILE_NORMAL, OPEN_ACTION_OPEN_IF_EXISTS,
                 OPEN_ACCESS_READONLY | OPEN_SHARE_DENYNONE, NULL ) != NO_ERROR ) return -1;
    DosRead( hf, buf, size - 1, &got );
    DosClose( hf );
    buf[got] = '\0';
    return (LONG)got;
}

static int exists( const char *name )
{
    FILESTATUS3 fs;
    return DosQueryPathInfo( (PSZ)name, FIL_STANDARD, &fs, sizeof( fs ) ) == NO_ERROR;
}

/* Wait up to `ms` for file `name`. */
static int wait_file( const char *name, ULONG ms )
{
    ULONG start = now_ms();
    while( !exists( name ) ) {
        if( now_ms() - start > ms ) return 0;
        DosSleep( 50 );
    }
    return 1;
}

/* ---------- handlers and exit-list routines of the children ---------- */

static VOID APIENTRY exit_routine( ULONG tc )
{
    char line[32];
    char *p = append_str( line, "tc=" );
    p = append_long( p, (LONG)tc );
    append_str( p, ";" );
    append_file( out_name, line );
    DosExitList( EXLST_EXIT, NULL );
}

/* Continues every XCPT_SIGNAL (records it). */
static ULONG APIENTRY continue_handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
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

/* "win": records XCPT_SIGNAL_INTR and continues; leaves
   XCPT_SIGNAL_KILLPROC to the default action (the exit list runs). */
static ULONG APIENTRY win_handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                                   PCONTEXTRECORD ctx, PVOID unused )
{
    (void)reg; (void)ctx; (void)unused;
    if( rep->ExceptionNum == XCPT_SIGNAL && rep->ExceptionInfo[0] == XCPT_SIGNAL_INTR ) {
        append_file( out_name, "intr;" );
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

static void ready( const char *mode )
{
    char name[32];
    file_name( name, mode, "RDY" );
    append_file( name, "ready" );
}

/* The command line's arguments: the second string at pib_pchcmd. */
static const char *command_args( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    own_pid = ppib->pib_ulpid;
    if( ppib->pib_pchcmd == NULL ) return "";
    return ppib->pib_pchcmd + strlen( ppib->pib_pchcmd ) + 1;
}

static APIRET start_child( const char *mode, PID *pid )
{
    PTIB        ptib;
    PPIB        ppib;
    char        me[CCHMAXPATH], args[CCHMAXPATH + 32], *p;
    RESULTCODES res;
    APIRET      rc;
    char        name[32];

    DosGetInfoBlocks( &ptib, &ppib );
    if( DosQueryModuleName( ppib->pib_hmte, sizeof( me ), me ) != NO_ERROR ) return 1;
    file_name( name, mode, "RDY" );
    DosDelete( (PSZ)name );
    file_name( name, mode, "OUT" );
    DosDelete( (PSZ)name );
    memset( args, 0, sizeof( args ) );
    p = append_str( args, me ) + 1;
    p = append_str( p, "child " );
    append_str( p, mode );
    rc = DosExecPgm( NULL, 0, EXEC_ASYNCRESULT, (PSZ)args, NULL, &res, (PSZ)me );
    /* EXEC_ASYNC(RESULT): the child's process id is in codeTerminate. */
    *pid = res.codeTerminate;
    return rc;
}

static int child_main( const char *mode )
{
    EXCEPTIONREGISTRATIONRECORD rec;
    char line[64], *p;
    ULONG times;
    APIRET rc;

    file_name( out_name, mode, "OUT" );
    rec.prev_structure = NULL;
    rec.ExceptionHandler = continue_handler;

    if( strcmp( mode, "exitlist" ) == 0 || strcmp( mode, "grand" ) == 0 ) {
        DosExitList( EXLST_ADD, (PFNEXITLIST)exit_routine );
        ready( mode );
        for( ;; ) DosSleep( 60000 );
    }
    if( strcmp( mode, "handler" ) == 0 ) {
        DosSetExceptionHandler( &rec );
        ready( mode );
        rc = DosSleep( 60000 );
        p = append_str( line, "sleep=" );
        p = append_long( p, (LONG)rc );
        p = append_str( p, ";sig=" );
        p = append_long( p, (LONG)got_signal );
        append_str( p, ";" );
        append_file( out_name, line );
        DosUnsetExceptionHandler( &rec );
        return 9;
    }
    if( strcmp( mode, "ignore" ) == 0 ) {
        DosSetExceptionHandler( &rec );
        DosExitList( EXLST_ADD, (PFNEXITLIST)exit_routine );
        ready( mode );
        for( ;; ) DosSleep( 1000 );
    }
    if( strcmp( mode, "tree" ) == 0 ) {
        PID grand, ended;
        RESULTCODES res;
        char name[32];
        DosExitList( EXLST_ADD, (PFNEXITLIST)exit_routine );
        if( start_child( "grand", &grand ) != NO_ERROR ) return 20;
        file_name( name, "grand", "RDY" );
        if( !wait_file( name, 10000 ) ) return 21;
        ready( mode );
        DosWaitChild( DCWA_PROCESS, DCWW_WAIT, &res, &ended, grand );
        for( ;; ) DosSleep( 60000 );
    }
    if( strcmp( mode, "kbd" ) == 0 ) {
        KEYINFO key;
        USHORT  krc;
        DosSetExceptionHandler( &rec );
        DosSetSignalExceptionFocus( SIG_SETFOCUS, &times );
        memset( &key, 0, sizeof( key ) );
        krc = KBD16CHARIN( &key, 0, 0 );
        check( "kbd: KbdCharIn after Ctrl+C", krc, NO_ERROR );
        check( "kbd: -> the next key", key.chChar, 'x' );
        check( "kbd: -> Ctrl+C was XCPT_SIGNAL_INTR", got_signal, XCPT_SIGNAL_INTR );
        DosUnsetExceptionHandler( &rec );
        return n_checks == n_passed ? 0 : 1;
    }
    if( strcmp( mode, "eof" ) == 0 ) {
        KEYINFO key;
        DosExitList( EXLST_ADD, (PFNEXITLIST)exit_routine );
        write_stdout( "kill_test: eof waiting\r\n" );
        KBD16CHARIN( &key, 0, 0 );
        write_stdout( "kill_test: eof FAIL KbdCharIn returned\r\n" );
        return 3;
    }
    if( strcmp( mode, "win" ) == 0 ) {
        KEYINFO key;
        rec.ExceptionHandler = win_handler;
        DosSetExceptionHandler( &rec );
        DosSetSignalExceptionFocus( SIG_SETFOCUS, &times );
        DosExitList( EXLST_ADD, (PFNEXITLIST)exit_routine );
        write_stdout( "kill_test: press Ctrl+C, then close the window\r\n" );
        for( ;; ) KBD16CHARIN( &key, 0, 0 );
    }
    return 99;
}

/* ---------- the parent ---------- */

static ULONG event_sem;

/* Thread: after `arg` ms, send XCPT_SIGNAL_BREAK to this process. */
static VOID APIENTRY signal_later( ULONG arg )
{
    DosSleep( arg );
    DosSendSignalException( own_pid, XCPT_SIGNAL_BREAK );
}

static HFILE pipe_write;

/* Thread: a signal, then a byte into the pipe. */
static VOID APIENTRY signal_then_write( ULONG arg )
{
    ULONG written;
    DosSleep( arg );
    DosSendSignalException( own_pid, XCPT_SIGNAL_BREAK );
    DosSleep( arg );
    DosWrite( pipe_write, "z", 1, &written );
}

static void interrupted_waits( void )
{
    EXCEPTIONREGISTRATIONRECORD rec;
    TID    tid;
    ULONG  times, start, elapsed, got;
    APIRET rc;
    HEV    hev;
    HFILE  pipe_read;
    char   byte = 0;

    rec.prev_structure = NULL;
    rec.ExceptionHandler = continue_handler;
    DosSetExceptionHandler( &rec );
    DosSetSignalExceptionFocus( SIG_SETFOCUS, &times );

    got_signal = 0;
    DosCreateThread( &tid, signal_later, 300, CREATE_READY | STACK_SPARSE, STACK_SIZE );
    start = now_ms();
    rc = DosSleep( 20000 );
    elapsed = now_ms() - start;
    check( "DosSleep interrupted by a signal", rc, ERROR_INTERRUPT );
    check( "-> the handler got XCPT_SIGNAL_BREAK", got_signal, XCPT_SIGNAL_BREAK );
    check( "-> well before its time", elapsed < 10000, 1 );
    DosWaitThread( &tid, DCWW_WAIT );

    got_signal = 0;
    DosCreateEventSem( NULL, &hev, 0, FALSE );
    event_sem = hev;
    DosCreateThread( &tid, signal_later, 300, CREATE_READY | STACK_SPARSE, STACK_SIZE );
    rc = DosWaitEventSem( hev, SEM_INDEFINITE_WAIT );
    check( "DosWaitEventSem interrupted by a signal", rc, ERROR_INTERRUPT );
    check( "-> the handler got it", got_signal, XCPT_SIGNAL_BREAK );
    DosWaitThread( &tid, DCWW_WAIT );
    DosCloseEventSem( hev );

    got_signal = 0;
    DosCreatePipe( &pipe_read, &pipe_write, 64 );
    DosCreateThread( &tid, signal_then_write, 300, CREATE_READY | STACK_SPARSE, STACK_SIZE );
    rc = DosRead( pipe_read, &byte, 1, &got );
    check( "DosRead of a pipe goes on after a signal", rc, NO_ERROR );
    check( "-> the handler got it first", got_signal, XCPT_SIGNAL_BREAK );
    check( "-> then the byte arrived", byte, 'z' );
    DosWaitThread( &tid, DCWW_WAIT );
    DosClose( pipe_read );
    DosClose( pipe_write );

    DosSetSignalExceptionFocus( SIG_UNSETFOCUS, &times );
    DosUnsetExceptionHandler( &rec );
}

/* Start child `mode`, wait until it is ready, kill it with `action`; the
   RESULTCODES of its end, -1 / -2 on failure. */
static LONG kill_child( const char *mode, ULONG action, RESULTCODES *res, APIRET *kill_rc )
{
    PID  pid, ended;
    char name[32];
    *kill_rc = 0xFFFF;
    if( start_child( mode, &pid ) != NO_ERROR ) return -1;
    file_name( name, mode, "RDY" );
    if( !wait_file( name, 10000 ) ) return -2;
    DosSleep( 100 );
    *kill_rc = DosKillProcess( action, pid );
    if( DosWaitChild( DCWA_PROCESS, DCWW_WAIT, res, &ended, pid ) != NO_ERROR ) return -3;
    return 0;
}

static void kill_children( void )
{
    RESULTCODES res;
    APIRET kill_rc;
    char   buf[128], name[32];
    ULONG  start;

    check( "DosKillProcess(bad action)", DosKillProcess( 2, own_pid ), ERROR_INVALID_DATA );
    check( "DosKillProcess(unknown pid)", DosKillProcess( DKP_PROCESS, 12345 ), ERROR_INVALID_PROCID );

    check( "exitlist child started and waited for", kill_child( "exitlist", DKP_PROCESS, &res, &kill_rc ), 0 );
    check( "-> DosKillProcess(DKP_PROCESS)", kill_rc, NO_ERROR );
    check( "-> codeTerminate TC_KILLPROCESS", res.codeTerminate, TC_KILLPROCESS );
    file_name( name, "exitlist", "OUT" );
    read_file( name, buf, sizeof( buf ) );
    check( "-> its exit-list routine ran with TC_KILLPROCESS", strcmp( buf, "tc=3;" ), 0 );

    check( "handler child started and waited for", kill_child( "handler", DKP_PROCESS, &res, &kill_rc ), 0 );
    check( "-> DosKillProcess", kill_rc, NO_ERROR );
    check( "-> it went on and ended by itself", res.codeResult, 9 );
    check( "-> codeTerminate TC_EXIT", res.codeTerminate, TC_EXIT );
    file_name( name, "handler", "OUT" );
    read_file( name, buf, sizeof( buf ) );
    check( "-> its DosSleep failed with ERROR_INTERRUPT after XCPT_SIGNAL_KILLPROC",
           strcmp( buf, "sleep=95;sig=3;" ), 0 );

    start = now_ms();
    check( "ignoring child started and waited for", kill_child( "ignore", DKP_PROCESS, &res, &kill_rc ), 0 );
    check( "-> DosKillProcess", kill_rc, NO_ERROR );
    check( "-> ended after the grace period", res.codeTerminate, TC_KILLPROCESS );
    check( "-> within a bounded time", now_ms() - start < KILL_GRACE_MAX_MS, 1 );

    check( "tree child started and waited for", kill_child( "tree", DKP_PROCESSTREE, &res, &kill_rc ), 0 );
    check( "-> DosKillProcess(DKP_PROCESSTREE)", kill_rc, NO_ERROR );
    check( "-> codeTerminate TC_KILLPROCESS", res.codeTerminate, TC_KILLPROCESS );
    file_name( name, "tree", "OUT" );
    read_file( name, buf, sizeof( buf ) );
    check( "-> its exit-list routine ran", strcmp( buf, "tc=3;" ), 0 );
    file_name( name, "grand", "OUT" );
    wait_file( name, 10000 );
    DosSleep( 200 );
    read_file( name, buf, sizeof( buf ) );
    check( "-> the grandchild's exit-list routine ran with TC_KILLPROCESS", strcmp( buf, "tc=3;" ), 0 );
}

int main( void )
{
    const char *args = command_args();
    char line[64], *p;

    if( strncmp( args, "child ", 6 ) == 0 ) return child_main( args + 6 );

    interrupted_waits();
    kill_children();

    p = append_str( line, "kill_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );
    return n_passed == n_checks ? 0 : 1;
}
