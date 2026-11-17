/*
 * thread_exit_test.c - ending a thread does not end the process
 *
 * Built twice from this source (see the Makefile):
 *
 * thread_exit_test.exe checks
 *   - a second thread calling DosExit(EXIT_THREAD, ...) ends only itself,
 *   - a thread function returning ends only that thread,
 *   - DosWaitThread on both (DCWW_WAIT, DCWW_NOWAIT, tid 0 = any thread,
 *     an ended thread reported once, thread 1 and the caller itself
 *     refused with ERROR_INVALID_THREADID),
 *   - a created thread's TIB2 holds its thread id; thread 1's is 1,
 *   - many threads created and ended one after another,
 * and the main thread continues afterwards; it ends the process with
 * DosExit(EXIT_PROCESS, 3), so the exit code is 3.
 *
 * thread_last_test.exe (LAST_THREAD defined): the main thread ends with
 * DosExit(EXIT_THREAD, 0) while a second thread still runs; that thread
 * then ends with DosExit(EXIT_THREAD, 5), the last thread of the process,
 * so the process ends with exit code 5.
 *
 * Each check is written to stdout:
 *
 *   thread_exit_test: <name>: ok
 *   thread_exit_test: <name>: FAIL got <value> want <value>
 *
 * followed by "thread_exit_test: done <passed>/<total>".
 */
#define INCL_DOSPROCESS
#define INCL_DOSSEMAPHORES
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define STACK_SIZE      0x4000
#define MANY_THREADS    100
#define EXIT_RESULT     3
#define LAST_RESULT     5

static int n_checks = 0;
static int n_passed = 0;

/* What the threads did, checked by the main thread afterwards. */
static volatile ULONG exited_ran;
static volatile ULONG returned_ran;
static volatile ULONG after_exit_ran;
static volatile ULONG thread_tib2_tid;
static volatile ULONG many_ran;
static HEV  go;

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
    char *p = append_str( line, "thread_exit_test: " );
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
    char *p = append_str( line, "thread_exit_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );
}

static ULONG own_tid( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    return ptib->tib_ptib2->tib2_ultid;
}

/* ---------- the threads ---------- */

static VOID APIENTRY exiting_thread( ULONG arg )
{
    (void)arg;
    thread_tib2_tid = own_tid();
    exited_ran = 1;
    DosExit( EXIT_THREAD, 0 );
    after_exit_ran = 1;          /* never reached */
}

static VOID APIENTRY returning_thread( ULONG arg )
{
    /* Wait until the main thread has seen this thread running. */
    DosWaitEventSem( go, 5000 );
    returned_ran = arg;
}

static VOID APIENTRY short_thread( ULONG arg )
{
    (void)arg;
    many_ran++;
}

#ifdef LAST_THREAD
static VOID APIENTRY last_thread( ULONG arg )
{
    (void)arg;
    DosSleep( 200 );
    write_stdout( "thread_exit_test: last thread ends the process\r\n" );
    DosExit( EXIT_THREAD, LAST_RESULT );
}
#endif

int main( void )
{
    TID   tid, tid2, tid3, any;
    ULONG rc, i, ok;

#ifdef LAST_THREAD
    rc = DosCreateThread( &tid, last_thread, 0, 0, STACK_SIZE );
    check( "DosCreateThread(last thread)", rc, NO_ERROR );
    write_stdout( "thread_exit_test: main thread ends\r\n" );
    DosExit( EXIT_THREAD, 0 );
    write_stdout( "thread_exit_test: main thread after DosExit: FAIL\r\n" );
    return 1;
#else
    DosCreateEventSem( NULL, &go, 0, FALSE );
    check( "thread 1's TIB2 thread id", own_tid(), 1 );

    /* A thread ending with DosExit(EXIT_THREAD). */
    rc = DosCreateThread( &tid2, exiting_thread, 0, 0, STACK_SIZE );
    check( "DosCreateThread(exiting thread)", rc, NO_ERROR );
    check( "-> not thread 1", tid2 != 1, 1 );
    tid = tid2;
    check( "DosWaitThread(exiting thread)", DosWaitThread( &tid, DCWW_WAIT ), NO_ERROR );
    check( "-> the thread ran", exited_ran, 1 );
    check( "-> nothing ran after its DosExit", after_exit_ran, 0 );
    check( "-> its TIB2 holds its thread id", thread_tib2_tid, tid2 );
    check( "main thread continues after a DosExit(EXIT_THREAD)", 1, 1 );
    tid = tid2;
    check( "DosWaitThread(ended thread) again", DosWaitThread( &tid, DCWW_WAIT ),
           ERROR_INVALID_THREADID );

    /* A thread function returning. */
    rc = DosCreateThread( &tid3, returning_thread, 42, 0, STACK_SIZE );
    check( "DosCreateThread(returning thread)", rc, NO_ERROR );
    tid = tid3;
    check( "DosWaitThread(running thread, DCWW_NOWAIT)", DosWaitThread( &tid, DCWW_NOWAIT ),
           ERROR_THREAD_NOT_TERMINATED );
    check( "-> tid kept", tid, tid3 );
    DosPostEventSem( go );
    any = 0;
    check( "DosWaitThread(any thread)", DosWaitThread( &any, DCWW_WAIT ), NO_ERROR );
    check( "-> the returning thread", any, tid3 );
    check( "-> it ran", returned_ran, 42 );
    check( "main thread continues after a thread returned", 1, 1 );

    /* What cannot be waited for. */
    tid = 1;
    check( "DosWaitThread(thread 1)", DosWaitThread( &tid, DCWW_WAIT ), ERROR_INVALID_THREADID );
    tid = own_tid();
    check( "DosWaitThread(own thread)", DosWaitThread( &tid, DCWW_WAIT ), ERROR_INVALID_THREADID );
    any = 0;
    check( "DosWaitThread(any thread) with none left", DosWaitThread( &any, DCWW_NOWAIT ),
           ERROR_INVALID_THREADID );

    /* Threads come and go; resources of ended threads are reused. */
    ok = 1;
    for( i = 0; i < MANY_THREADS; i++ ) {
        if( DosCreateThread( &tid, short_thread, 0, 0, STACK_SIZE ) != NO_ERROR ) { ok = 0; break; }
        if( DosWaitThread( &tid, DCWW_WAIT ) != NO_ERROR ) { ok = 0; break; }
    }
    check( "100 threads created and ended one after another", ok, 1 );
    check( "-> all of them ran", many_ran, MANY_THREADS );

    done();
    DosExit( EXIT_PROCESS, EXIT_RESULT );
    return 1;
#endif
}
