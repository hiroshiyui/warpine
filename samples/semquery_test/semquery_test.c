/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * semquery_test.c - OS/2 2.0 semaphore calls beside create / wait / post:
 *
 *   DosResetEventSem / DosQueryEventSem   post counts, ERROR_ALREADY_RESET,
 *                                         ERROR_ALREADY_POSTED still counts
 *   DosQueryMutexSem                      owner pid / tid, request count
 *   DosOpenMuxWaitSem                     by name and by handle
 *   DosAddMuxWaitSem / DosDeleteMuxWaitSem ERROR_DUPLICATE_HANDLE,
 *                                         ERROR_WRONG_TYPE, 64 at most
 *                                         (ERROR_TOO_MANY_SEMAPHORES),
 *                                         ERROR_EMPTY_MUXWAIT
 *   DosQueryMuxWaitSem                    SEMRECORDs, flags,
 *                                         ERROR_PARAM_TOO_SMALL + count
 *   DosWaitMuxWaitSem                     any / all, takes the mutexes
 *
 * Each check is written to stdout:
 *
 *   semquery_test: <name>: ok
 *   semquery_test: <name>: FAIL got <value> want <value>
 *
 * followed by "semquery_test: done <passed>/<total>".
 */
#define INCL_DOSPROCESS
#define INCL_DOSSEMAPHORES
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define STACK_SIZE  0x4000
#define MAX_MUX     64

static int n_checks = 0;
static int n_passed = 0;

static HMTX thread_mtx;
static HEV  thread_has_it, thread_may_go;

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
    char  line[160];
    char *p = append_str( line, "semquery_test: " );
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
    char *p = append_str( line, "semquery_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );
}

/* ---------- event semaphores ---------- */

static void event_counts( void )
{
    HEV   hev, opened;
    ULONG count = 99;

    check( "DosCreateEventSem(named)", DosCreateEventSem( "\\SEM32\\SEMQUERY\\EV", &hev, 0, FALSE ), NO_ERROR );
    check( "DosQueryEventSem(new)", DosQueryEventSem( hev, &count ), NO_ERROR );
    check( "-> post count 0", count, 0 );
    count = 99;
    check( "DosResetEventSem(not posted)", DosResetEventSem( hev, &count ), ERROR_ALREADY_RESET );
    check( "DosPostEventSem", DosPostEventSem( hev ), NO_ERROR );
    check( "DosPostEventSem(again)", DosPostEventSem( hev ), ERROR_ALREADY_POSTED );
    check( "DosPostEventSem(third)", DosPostEventSem( hev ), ERROR_ALREADY_POSTED );
    check( "DosQueryEventSem(posted)", DosQueryEventSem( hev, &count ), NO_ERROR );
    check( "-> every post counted", count, 3 );
    count = 0;
    check( "DosResetEventSem(posted)", DosResetEventSem( hev, &count ), NO_ERROR );
    check( "-> the count before the reset", count, 3 );
    DosQueryEventSem( hev, &count );
    check( "-> post count now 0", count, 0 );
    check( "DosWaitEventSem(reset, 0 ms)", DosWaitEventSem( hev, SEM_IMMEDIATE_RETURN ), ERROR_TIMEOUT );
    check( "DosQueryEventSem(bad handle)", DosQueryEventSem( 0xBADBAD, &count ), ERROR_INVALID_HANDLE );
    check( "DosResetEventSem(bad handle)", DosResetEventSem( 0xBADBAD, &count ), ERROR_INVALID_HANDLE );
    opened = 0;
    check( "DosOpenEventSem(by name)", DosOpenEventSem( "\\SEM32\\SEMQUERY\\EV", &opened ), NO_ERROR );
    check( "-> the same handle", opened == hev, 1 );
    check( "DosCloseEventSem(opened)", DosCloseEventSem( opened ), NO_ERROR );
    check( "DosCloseEventSem(created)", DosCloseEventSem( hev ), NO_ERROR );
}

/* ---------- mutex semaphores ---------- */

static VOID APIENTRY mutex_thread( ULONG arg )
{
    (void)arg;
    DosRequestMutexSem( thread_mtx, SEM_INDEFINITE_WAIT );
    DosPostEventSem( thread_has_it );
    DosWaitEventSem( thread_may_go, 5000 );
    DosReleaseMutexSem( thread_mtx );
}

static void mutex_query( void )
{
    HMTX  hmtx, owned;
    PID   pid = 99;
    TID   tid = 99, thread;
    ULONG count = 99;
    PTIB  ptib;
    PPIB  ppib;

    DosGetInfoBlocks( &ptib, &ppib );
    check( "DosCreateMutexSem", DosCreateMutexSem( NULL, &hmtx, 0, FALSE ), NO_ERROR );
    check( "DosQueryMutexSem(unowned)", DosQueryMutexSem( hmtx, &pid, &tid, &count ), NO_ERROR );
    check( "-> request count 0", count, 0 );
    DosRequestMutexSem( hmtx, SEM_INDEFINITE_WAIT );
    DosRequestMutexSem( hmtx, SEM_INDEFINITE_WAIT );
    check( "DosQueryMutexSem(owned twice)", DosQueryMutexSem( hmtx, &pid, &tid, &count ), NO_ERROR );
    check( "-> owner pid is ours", pid, ppib->pib_ulpid );
    check( "-> owner tid 1", tid, 1 );
    check( "-> request count 2", count, 2 );
    DosReleaseMutexSem( hmtx );
    DosQueryMutexSem( hmtx, &pid, &tid, &count );
    check( "-> request count 1 after a release", count, 1 );
    DosReleaseMutexSem( hmtx );
    DosQueryMutexSem( hmtx, &pid, &tid, &count );
    check( "-> request count 0 after both", count, 0 );
    check( "DosReleaseMutexSem(not owner)", DosReleaseMutexSem( hmtx ), ERROR_NOT_OWNER );

    check( "DosCreateMutexSem(owned)", DosCreateMutexSem( NULL, &owned, 0, TRUE ), NO_ERROR );
    DosQueryMutexSem( owned, &pid, &tid, &count );
    check( "-> owned by thread 1", tid, 1 );
    check( "-> request count 1", count, 1 );
    DosReleaseMutexSem( owned );
    DosCloseMutexSem( owned );

    /* Owned by another thread. */
    thread_mtx = hmtx;
    DosCreateEventSem( NULL, &thread_has_it, 0, FALSE );
    DosCreateEventSem( NULL, &thread_may_go, 0, FALSE );
    check( "DosCreateThread", DosCreateThread( &thread, mutex_thread, 0, 0, STACK_SIZE ), NO_ERROR );
    DosWaitEventSem( thread_has_it, 5000 );
    DosQueryMutexSem( hmtx, &pid, &tid, &count );
    check( "-> owned by the other thread", tid, thread );
    check( "DosRequestMutexSem(owned elsewhere, 0 ms)", DosRequestMutexSem( hmtx, SEM_IMMEDIATE_RETURN ),
           ERROR_TIMEOUT );
    DosPostEventSem( thread_may_go );
    DosWaitThread( &thread, DCWW_WAIT );
    DosQueryMutexSem( hmtx, &pid, &tid, &count );
    check( "-> released when the thread let go", count, 0 );
    check( "DosQueryMutexSem(bad handle)", DosQueryMutexSem( 0xBADBAD, &pid, &tid, &count ),
           ERROR_INVALID_HANDLE );
    DosCloseMutexSem( hmtx );
}

/* ---------- muxwait semaphores ---------- */

static void muxwait_lists( void )
{
    HEV       ev[MAX_MUX + 1], priv;
    HMTX      m1, m2;
    HMUX      hmux, opened, mmux;
    SEMRECORD rec[MAX_MUX + 1], got[4];
    ULONG     n, attr, user, i, ok;
    PID       pid;
    TID       tid;

    /* Shared events: a named (so shared) list holds shared semaphores only. */
    for( i = 0; i <= MAX_MUX; i++ ) {
        DosCreateEventSem( NULL, &ev[i], DC_SEM_SHARED, FALSE );
        rec[i].hsemCur = (HSEM)ev[i];
        rec[i].ulUser = 100 + i;
    }
    DosCreateMutexSem( NULL, &m1, 0, FALSE );
    DosCreateMutexSem( NULL, &m2, 0, FALSE );

    check( "DosCreateMuxWaitSem(65 records)",
           DosCreateMuxWaitSem( NULL, &hmux, MAX_MUX + 1, rec, DCMW_WAIT_ANY ), ERROR_TOO_MANY_SEMAPHORES );
    got[0] = rec[0]; got[1] = rec[0];
    check( "DosCreateMuxWaitSem(same semaphore twice)",
           DosCreateMuxWaitSem( NULL, &hmux, 2, got, DCMW_WAIT_ANY ), ERROR_DUPLICATE_HANDLE );
    got[1].hsemCur = (HSEM)m1;
    check( "DosCreateMuxWaitSem(event and mutex)",
           DosCreateMuxWaitSem( NULL, &hmux, 2, got, DCMW_WAIT_ANY ), ERROR_WRONG_TYPE );

    check( "DosCreateMuxWaitSem(named, 2 events)",
           DosCreateMuxWaitSem( "\\SEM32\\SEMQUERY\\MUX", &hmux, 2, rec, DCMW_WAIT_ANY ), NO_ERROR );
    n = 1;
    check( "DosQueryMuxWaitSem(room for 1)", DosQueryMuxWaitSem( hmux, &n, got, &attr ), ERROR_PARAM_TOO_SMALL );
    check( "-> records needed", n, 2 );
    n = 4;
    check( "DosQueryMuxWaitSem", DosQueryMuxWaitSem( hmux, &n, got, &attr ), NO_ERROR );
    check( "-> 2 records", n, 2 );
    check( "-> first record's semaphore", got[0].hsemCur == (HSEM)ev[0], 1 );
    check( "-> first record's user value", got[0].ulUser, 100 );
    check( "-> second record's semaphore", got[1].hsemCur == (HSEM)ev[1], 1 );
    check( "-> second record's user value", got[1].ulUser, 101 );
    check( "-> DCMW_WAIT_ANY", ( attr & ( DCMW_WAIT_ANY | DCMW_WAIT_ALL ) ), DCMW_WAIT_ANY );
    check( "-> named: shared", ( attr & DC_SEM_SHARED ), DC_SEM_SHARED );

    DosCreateEventSem( NULL, &priv, 0, FALSE );
    got[0].hsemCur = (HSEM)priv; got[0].ulUser = 7;
    check( "DosAddMuxWaitSem(a private event to a shared list)", DosAddMuxWaitSem( hmux, &got[0] ),
           ERROR_WRONG_TYPE );
    DosCloseEventSem( priv );
    check( "DosAddMuxWaitSem(already in the list)", DosAddMuxWaitSem( hmux, &rec[1] ), ERROR_DUPLICATE_HANDLE );
    got[0].hsemCur = (HSEM)m1; got[0].ulUser = 7;
    check( "DosAddMuxWaitSem(a mutex to an event list)", DosAddMuxWaitSem( hmux, &got[0] ), ERROR_WRONG_TYPE );
    got[0].hsemCur = (HSEM)hmux;
    check( "DosAddMuxWaitSem(a muxwait semaphore)", DosAddMuxWaitSem( hmux, &got[0] ), ERROR_WRONG_TYPE );
    ok = 1;
    for( i = 2; i < MAX_MUX; i++ ) {
        if( DosAddMuxWaitSem( hmux, &rec[i] ) != NO_ERROR ) ok = 0;
    }
    check( "DosAddMuxWaitSem up to 64", ok, 1 );
    check( "DosAddMuxWaitSem(65th)", DosAddMuxWaitSem( hmux, &rec[MAX_MUX] ), ERROR_TOO_MANY_SEMAPHORES );
    n = 4;
    check( "DosQueryMuxWaitSem(64, room for 4)", DosQueryMuxWaitSem( hmux, &n, got, &attr ), ERROR_PARAM_TOO_SMALL );
    check( "-> records needed", n, MAX_MUX );

    /* Waiting for any: the posted one's user value. */
    DosPostEventSem( ev[5] );
    user = 0;
    check( "DosWaitMuxWaitSem(any, one posted)", DosWaitMuxWaitSem( hmux, 1000, &user ), NO_ERROR );
    check( "-> its user value", user, 105 );

    opened = 0;
    check( "DosOpenMuxWaitSem(by name)", DosOpenMuxWaitSem( "\\SEM32\\SEMQUERY\\MUX", &opened ), NO_ERROR );
    check( "-> the same handle", opened == hmux, 1 );
    check( "DosOpenMuxWaitSem(by handle)", DosOpenMuxWaitSem( NULL, &opened ), NO_ERROR );
    check( "DosOpenMuxWaitSem(unknown name)", DosOpenMuxWaitSem( "\\SEM32\\SEMQUERY\\NONE", &opened ),
           ERROR_SEM_NOT_FOUND );
    check( "DosCloseMuxWaitSem(1)", DosCloseMuxWaitSem( hmux ), NO_ERROR );
    check( "DosCloseMuxWaitSem(2)", DosCloseMuxWaitSem( hmux ), NO_ERROR );
    n = 4;
    check( "-> still open once", DosQueryMuxWaitSem( hmux, &n, got, &attr ), ERROR_PARAM_TOO_SMALL );
    check( "DosCloseMuxWaitSem(3)", DosCloseMuxWaitSem( hmux ), NO_ERROR );
    check( "-> gone after the last close", DosCloseMuxWaitSem( hmux ), ERROR_INVALID_HANDLE );

    /* Mutexes: waiting for all takes them all; deleting empties the list. */
    got[0].hsemCur = (HSEM)m1; got[0].ulUser = 1;
    got[1].hsemCur = (HSEM)m2; got[1].ulUser = 2;
    check( "DosCreateMuxWaitSem(2 mutexes, all)", DosCreateMuxWaitSem( NULL, &mmux, 2, got, DCMW_WAIT_ALL ), NO_ERROR );
    check( "DosWaitMuxWaitSem(all)", DosWaitMuxWaitSem( mmux, 1000, &user ), NO_ERROR );
    DosQueryMutexSem( m1, &pid, &tid, &n );
    check( "-> first mutex taken", n, 1 );
    DosQueryMutexSem( m2, &pid, &tid, &n );
    check( "-> second mutex taken", n, 1 );
    DosReleaseMutexSem( m1 );
    DosReleaseMutexSem( m2 );
    check( "DosDeleteMuxWaitSem(first)", DosDeleteMuxWaitSem( mmux, (HSEM)m1 ), NO_ERROR );
    check( "DosDeleteMuxWaitSem(second)", DosDeleteMuxWaitSem( mmux, (HSEM)m2 ), NO_ERROR );
    check( "DosDeleteMuxWaitSem(empty list)", DosDeleteMuxWaitSem( mmux, (HSEM)m2 ), ERROR_EMPTY_MUXWAIT );
    n = 4;
    check( "DosQueryMuxWaitSem(empty list)", DosQueryMuxWaitSem( mmux, &n, got, &attr ), NO_ERROR );
    check( "-> 0 records", n, 0 );
    check( "-> DCMW_WAIT_ALL", ( attr & ( DCMW_WAIT_ANY | DCMW_WAIT_ALL ) ), DCMW_WAIT_ALL );
    DosCloseMuxWaitSem( mmux );
}

int main( void )
{
    event_counts();
    mutex_query();
    muxwait_lists();
    done();
    return 0;
}
