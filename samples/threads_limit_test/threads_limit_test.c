/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * threads_limit_test.c - the THREADS= limit of CONFIG.SYS
 *
 * OS/2 bounds the threads of the whole system by THREADS= (every process
 * needs one for its thread 1). Warpine counts them for the whole session,
 * across its processes. This program, run with a limit L (THREADS= in
 * C:\CONFIG.SYS or WARPINE_THREADS):
 *
 *   1. creates threads until DosCreateThread fails; it must fail with
 *      ERROR_MAX_THRDS_REACHED (164). Writes "created <n> threads" (n = L-1).
 *   2. ends them, keeps 9 running (10 with thread 1) and runs a child that
 *      creates threads until it fails; the child's exit code is its count
 *      (L-11: the limit is shared by both processes).
 *   3. fills the session again, ends 2 threads and starts children
 *      asynchronously: two start, the third fails with ERROR_NO_PROC_SLOTS
 *      (89); so does a synchronous start.
 *   4. ends its threads, waits for the children and runs a child that ends
 *      with 7: the slots of ended threads and processes come back.
 *
 * Run as "threads_limit_test cap" with a limit above a process's own
 * (WARPINE_THREADS=4095), it creates threads until DosCreateThread fails,
 * twice, and writes "process cap: created <n> rc=<rc>" each time: Warpine
 * gives a process at most 738 threads (thread 1 and 737 more, ADR-0115),
 * and the failure is ERROR_MAX_THRDS_REACHED (164).
 *
 * One "ok ..." or "FAIL ..." line per check; exit code 0 when all passed.
 *
 * A child knows its mode from its command line, read with DosGetInfoBlocks
 * (pib_pchcmd: the program name, NUL, the arguments) - not from argv.
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define MAX_T 4100

static volatile int stop_flag[MAX_T];
static TID          tids[MAX_T];
static int          failures;

static char *append_str( char *p, const char *s )
{
    while( *s ) *p++ = *s++;
    *p = '\0';
    return p;
}

static char *append_long( char *p, ULONG u )
{
    char tmp[12];
    int  i = 0;
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

static void report( int ok, const char *what, ULONG value )
{
    char line[160], *p;
    if( !ok ) failures++;
    p = append_str( line, ok ? "ok " : "FAIL " );
    p = append_str( p, what );
    p = append_long( p, value );
    append_str( p, "\r\n" );
    write_stdout( line );
}

static void APIENTRY worker( ULONG i )
{
    while( !stop_flag[i] ) DosSleep( 10 );
}

/* The cap mode's workers block on one event semaphore instead of polling:
   hundreds of polling threads would load the machine. */
static HEV gate;

static void APIENTRY blocked_worker( ULONG i )
{
    (void)i;
    DosWaitEventSem( gate, SEM_INDEFINITE_WAIT );
}

/* Create blocked workers until DosCreateThread fails; returns the number
   created, *prc the failing return code. */
static int fill_blocked( APIRET *prc )
{
    int    n = 0;
    APIRET rc = NO_ERROR;
    while( n < MAX_T ) {
        rc = DosCreateThread( &tids[n], (PFNTHREAD)blocked_worker, n, CREATE_READY, 16384 );
        if( rc != NO_ERROR ) break;
        n++;
    }
    *prc = rc;
    return n;
}

/* Release the blocked workers, wait for them, and close the gate again. */
static void end_blocked( int n )
{
    ULONG posts;
    int   i;
    DosPostEventSem( gate );
    for( i = 0; i < n; i++ ) DosWaitThread( &tids[i], DCWW_WAIT );
    DosResetEventSem( gate, &posts );
}

/* Create threads from index `from` until DosCreateThread fails; returns
   the number created, *prc the failing return code. */
static int fill( int from, APIRET *prc )
{
    int    n = from;
    APIRET rc = NO_ERROR;
    while( n < MAX_T ) {
        stop_flag[n] = 0;
        rc = DosCreateThread( &tids[n], (PFNTHREAD)worker, n, CREATE_READY, 16384 );
        if( rc != NO_ERROR ) break;
        n++;
    }
    *prc = rc;
    return n - from;
}

static void end_threads( int from, int to )
{
    int i;
    for( i = from; i < to; i++ ) stop_flag[i] = 1;
    for( i = from; i < to; i++ ) DosWaitThread( &tids[i], DCWW_WAIT );
}

static char path[CCHMAXPATH];

static APIRET start( ULONG flag, const char *mode, RESULTCODES *res )
{
    char   args[CCHMAXPATH + 16], fail[64], *p;
    memset( args, 0, sizeof( args ) );
    p = append_str( args, path ) + 1;
    append_str( p, mode );
    return DosExecPgm( fail, sizeof( fail ), flag, args, NULL, res, path );
}

/* The argument string of the command line. */
static const char *arguments( void )
{
    PTIB ptib;
    PPIB ppib;
    const char *p;
    DosGetInfoBlocks( &ptib, &ppib );
    p = ppib->pib_pchcmd;
    if( p == NULL ) return "";
    while( *p ) p++;
    p++;
    while( *p == ' ' ) p++;
    return p;
}

int main( void )
{
    PTIB        ptib;
    PPIB        ppib;
    RESULTCODES res, kids[3];
    APIRET      rc;
    PID         pid;
    const char *mode = arguments();
    int         n, i, started;

    if( strncmp( mode, "count", 5 ) == 0 ) {
        n = fill( 0, &rc );
        end_threads( 0, n );
        return rc == ERROR_MAX_THRDS_REACHED ? n : 255;
    }
    if( strncmp( mode, "sleep", 5 ) == 0 ) {
        DosSleep( 2000 );
        return 0;
    }
    if( strncmp( mode, "exit7", 5 ) == 0 ) return 7;
    if( strncmp( mode, "cap", 3 ) == 0 ) {
        if( DosCreateEventSem( NULL, &gate, 0, FALSE ) != NO_ERROR ) return 255;
        for( i = 0; i < 2; i++ ) {
            n = fill_blocked( &rc );
            report( rc == ERROR_MAX_THRDS_REACHED, "process cap rc=", rc );
            report( n > 0, "process cap: created threads=", n );
            end_blocked( n );
        }
        write_stdout( failures ? "threads_limit_test: FAILED\r\n" : "threads_limit_test: all passed\r\n" );
        return failures ? 1 : 0;
    }

    DosGetInfoBlocks( &ptib, &ppib );
    if( DosQueryModuleName( ppib->pib_hmte, sizeof( path ), path ) != NO_ERROR ) return 255;

    /* 1. Threads until the limit. */
    n = fill( 0, &rc );
    report( rc == ERROR_MAX_THRDS_REACHED, "DosCreateThread at the limit rc=", rc );
    report( n > 0, "created threads=", n );
    end_threads( 0, n );

    /* 2. The limit covers the child too. */
    for( i = 0; i < 9; i++ ) {
        stop_flag[i] = 0;
        DosCreateThread( &tids[i], (PFNTHREAD)worker, i, CREATE_READY, 16384 );
    }
    rc = start( EXEC_SYNC, "count", &res );
    report( rc == NO_ERROR && res.codeResult != 255, "child created threads=", res.codeResult );

    /* 3. No slot for another process. */
    n = 9 + fill( 9, &rc );
    report( rc == ERROR_MAX_THRDS_REACHED, "session full again rc=", rc );
    end_threads( n - 2, n );
    n -= 2;
    started = 0;
    for( i = 0; i < 3; i++ ) {
        rc = start( EXEC_ASYNC, "sleep", &kids[i] );
        if( rc != NO_ERROR ) break;
        started++;
    }
    report( started == 2, "children started=", started );
    report( rc == ERROR_NO_PROC_SLOTS, "DosExecPgm without a slot rc=", rc );
    rc = start( EXEC_SYNC, "exit7", &res );
    report( rc == ERROR_NO_PROC_SLOTS, "synchronous DosExecPgm without a slot rc=", rc );

    /* 4. Slots come back. */
    end_threads( 0, n );
    for( i = 0; i < started; i++ ) {
        DosWaitChild( DCWA_PROCESS, DCWW_WAIT, &res, &pid, kids[i].codeTerminate );
    }
    rc = start( EXEC_SYNC, "exit7", &res );
    report( rc == NO_ERROR && res.codeResult == 7, "child after the slots came back result=",
            rc == NO_ERROR ? res.codeResult : rc );
    n = fill( 0, &rc );
    report( rc == ERROR_MAX_THRDS_REACHED, "threads again rc=", rc );
    report( n > 0, "created again threads=", n );
    end_threads( 0, n );

    write_stdout( failures ? "threads_limit_test: FAILED\r\n" : "threads_limit_test: all passed\r\n" );
    return failures ? 1 : 0;
}
