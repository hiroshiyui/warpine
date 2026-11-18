/*
 * semfollow_test.c - semaphore, pipe-semaphore and suballocation details:
 *
 *   DCE_POSTONE / DCE_AUTORESET   event semaphores that take their posts
 *                                 when they release a wait
 *   ERROR_SEM_OWNER_DIED          a mutex whose owner thread ended:
 *                                 DosQueryMutexSem, the next request, a
 *                                 waiter blocked meanwhile, muxwait lists
 *   ERROR_SEM_BUSY                closing an event, mutex or muxwait
 *                                 semaphore another thread waits on
 *   DosWaitMuxWaitSem             woken by a post from another thread
 *   DosSetNPipeSem /              a semaphore on a named-pipe end, posted
 *   DosQueryNPipeSemState         for data and for the other end's close;
 *                                 PIPESEMSTATE records
 *   DosFreeMem without            a suballocation pool's memory freed
 *   DosSubUnsetMem                without unsetting it can hold a new pool
 *
 * Each check is written to stdout:
 *
 *   semfollow_test: <name>: ok
 *   semfollow_test: <name>: FAIL got <value> want <value>
 *
 * followed by "semfollow_test: done <passed>/<total>".
 */
#define INCL_DOSPROCESS
#define INCL_DOSSEMAPHORES
#define INCL_DOSNMPIPES
#define INCL_DOSFILEMGR
#define INCL_DOSMEMMGR
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define STACK_SIZE  0x4000
#define PIPE_NAME   "\\PIPE\\SEMFOLLOW\\STATE"

#ifndef ERROR_SEM_OWNER_DIED
#define ERROR_SEM_OWNER_DIED 105
#endif
#ifndef ERROR_SEM_BUSY
#define ERROR_SEM_BUSY 301
#endif

static int n_checks = 0;
static int n_passed = 0;

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
    char *p = append_str( line, "semfollow_test: " );
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
    char *p = append_str( line, "semfollow_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );
}

/* ---------- DCE_POSTONE / DCE_AUTORESET ---------- */

static ULONG post_count( HEV hev )
{
    ULONG n = 99;
    DosQueryEventSem( hev, &n );
    return n;
}

static void event_flags( void )
{
    HEV       one, autor, bad;
    HMUX      hmux;
    SEMRECORD rec[1];
    ULONG     user = 0;

    check( "DosCreateEventSem(unknown flag)", DosCreateEventSem( NULL, &bad, 0x0100, FALSE ),
           ERROR_INVALID_PARAMETER );
    check( "DosCreateEventSem(DCE_POSTONE)", DosCreateEventSem( NULL, &one, DCE_POSTONE, FALSE ), NO_ERROR );
    check( "DosCreateEventSem(DCE_AUTORESET)", DosCreateEventSem( NULL, &autor, DCE_AUTORESET, FALSE ),
           NO_ERROR );

    DosPostEventSem( one );
    DosPostEventSem( one );
    check( "POSTONE: first wait", DosWaitEventSem( one, SEM_IMMEDIATE_RETURN ), NO_ERROR );
    check( "-> one post left", post_count( one ), 1 );
    check( "POSTONE: second wait", DosWaitEventSem( one, SEM_IMMEDIATE_RETURN ), NO_ERROR );
    check( "POSTONE: third wait times out", DosWaitEventSem( one, SEM_IMMEDIATE_RETURN ), ERROR_TIMEOUT );

    DosPostEventSem( autor );
    DosPostEventSem( autor );
    check( "AUTORESET: wait", DosWaitEventSem( autor, SEM_IMMEDIATE_RETURN ), NO_ERROR );
    check( "-> reset", post_count( autor ), 0 );

    rec[0].hsemCur = (HSEM)autor;
    rec[0].ulUser = 5;
    DosCreateMuxWaitSem( NULL, &hmux, 1, rec, DCMW_WAIT_ANY );
    DosPostEventSem( autor );
    check( "AUTORESET in a muxwait list", DosWaitMuxWaitSem( hmux, SEM_IMMEDIATE_RETURN, &user ), NO_ERROR );
    check( "-> user value", user, 5 );
    check( "-> reset by the muxwait wait", post_count( autor ), 0 );

    DosCloseMuxWaitSem( hmux );
    DosCloseEventSem( one );
    DosCloseEventSem( autor );
}

/* ---------- ERROR_SEM_OWNER_DIED ---------- */

static HMTX owner_mtx;
static HEV  owner_has_it, owner_may_go;

static void APIENTRY owner_thread( ULONG arg )
{
    DosRequestMutexSem( owner_mtx, SEM_INDEFINITE_WAIT );
    DosRequestMutexSem( owner_mtx, SEM_INDEFINITE_WAIT );
    DosPostEventSem( owner_has_it );
    DosWaitEventSem( owner_may_go, SEM_INDEFINITE_WAIT );
    if( arg ) DosSleep( 200 );      /* let thread 1 block on the mutex */
    DosExit( EXIT_THREAD, 0 );      /* without releasing it */
}

static TID start_owner( HMTX hmtx, ULONG delay )
{
    TID   tid = 0;
    ULONG n;
    owner_mtx = hmtx;
    DosResetEventSem( owner_has_it, &n );
    DosResetEventSem( owner_may_go, &n );
    DosCreateThread( &tid, owner_thread, delay, 0, STACK_SIZE );
    DosWaitEventSem( owner_has_it, 5000 );
    return tid;
}

static void owner_died( void )
{
    HMTX      hmtx, m2;
    HMUX      hmux;
    SEMRECORD rec[2];
    TID       tid, dead;
    PID       pid;
    ULONG     count, user = 0;

    DosCreateEventSem( NULL, &owner_has_it, 0, FALSE );
    DosCreateEventSem( NULL, &owner_may_go, 0, FALSE );
    DosCreateMutexSem( NULL, &hmtx, 0, FALSE );

    /* The owner ends; nobody waits. */
    dead = start_owner( hmtx, 0 );
    DosPostEventSem( owner_may_go );
    tid = dead;
    DosWaitThread( &tid, DCWW_WAIT );
    tid = 0; count = 99;
    check( "DosQueryMutexSem(owner ended)", DosQueryMutexSem( hmtx, &pid, &tid, &count ),
           ERROR_SEM_OWNER_DIED );
    check( "-> the dead owner's thread", tid, dead );
    check( "DosRequestMutexSem(owner ended)", DosRequestMutexSem( hmtx, SEM_IMMEDIATE_RETURN ),
           ERROR_SEM_OWNER_DIED );
    DosQueryMutexSem( hmtx, &pid, &tid, &count );
    check( "-> now owned by thread 1", tid, 1 );
    check( "-> request count 1", count, 1 );
    check( "DosRequestMutexSem(again)", DosRequestMutexSem( hmtx, SEM_IMMEDIATE_RETURN ), NO_ERROR );
    check( "DosReleaseMutexSem", DosReleaseMutexSem( hmtx ), NO_ERROR );
    check( "DosReleaseMutexSem(last)", DosReleaseMutexSem( hmtx ), NO_ERROR );

    /* Thread 1 is blocked on the mutex when the owner ends. */
    dead = start_owner( hmtx, 1 );
    DosPostEventSem( owner_may_go );
    check( "DosRequestMutexSem(blocked, owner ends)", DosRequestMutexSem( hmtx, 10000 ),
           ERROR_SEM_OWNER_DIED );
    DosReleaseMutexSem( hmtx );
    tid = dead;
    DosWaitThread( &tid, DCWW_WAIT );

    /* A muxwait list of mutexes. */
    DosCreateMutexSem( NULL, &m2, 0, FALSE );
    rec[0].hsemCur = (HSEM)hmtx; rec[0].ulUser = 1;
    rec[1].hsemCur = (HSEM)m2;   rec[1].ulUser = 2;
    DosCreateMuxWaitSem( NULL, &hmux, 2, rec, DCMW_WAIT_ALL );
    dead = start_owner( hmtx, 0 );
    DosPostEventSem( owner_may_go );
    tid = dead;
    DosWaitThread( &tid, DCWW_WAIT );
    check( "DosWaitMuxWaitSem(a mutex's owner ended)", DosWaitMuxWaitSem( hmux, 1000, &user ),
           ERROR_SEM_OWNER_DIED );
    check( "-> last record's user value", user, 2 );
    check( "-> the mutex was taken", DosReleaseMutexSem( hmtx ), NO_ERROR );
    DosReleaseMutexSem( m2 );

    DosCloseMuxWaitSem( hmux );
    DosCloseMutexSem( m2 );
    DosCloseMutexSem( hmtx );
}

/* ---------- ERROR_SEM_BUSY and muxwait wake-ups ---------- */

static HEV  busy_ev;
static HMTX busy_mtx;
static HMUX busy_mux;
static volatile ULONG busy_rc;

static void APIENTRY event_waiter( ULONG arg )
{
    busy_rc = DosWaitEventSem( busy_ev, 10000 );
}

static void APIENTRY mutex_waiter( ULONG arg )
{
    busy_rc = DosRequestMutexSem( busy_mtx, 10000 );
    if( busy_rc == NO_ERROR ) DosReleaseMutexSem( busy_mtx );
}

static void APIENTRY mux_waiter( ULONG arg )
{
    ULONG user = 0;
    busy_rc = DosWaitMuxWaitSem( busy_mux, 10000, &user );
    if( user != 7 ) busy_rc = 9999;
}

static void busy_close( void )
{
    TID       tid;
    SEMRECORD rec[1];

    DosCreateEventSem( NULL, &busy_ev, 0, FALSE );
    busy_rc = 99;
    DosCreateThread( &tid, event_waiter, 0, 0, STACK_SIZE );
    DosSleep( 200 );
    check( "DosCloseEventSem(a thread waits)", DosCloseEventSem( busy_ev ), ERROR_SEM_BUSY );
    DosPostEventSem( busy_ev );
    DosWaitThread( &tid, DCWW_WAIT );
    check( "-> the waiter was released", busy_rc, NO_ERROR );
    check( "DosCloseEventSem(nobody waits)", DosCloseEventSem( busy_ev ), NO_ERROR );

    DosCreateMutexSem( NULL, &busy_mtx, 0, TRUE );
    busy_rc = 99;
    DosCreateThread( &tid, mutex_waiter, 0, 0, STACK_SIZE );
    DosSleep( 200 );
    check( "DosCloseMutexSem(a thread waits)", DosCloseMutexSem( busy_mtx ), ERROR_SEM_BUSY );
    DosReleaseMutexSem( busy_mtx );
    DosWaitThread( &tid, DCWW_WAIT );
    check( "-> the waiter got it", busy_rc, NO_ERROR );
    check( "DosCloseMutexSem(nobody waits)", DosCloseMutexSem( busy_mtx ), NO_ERROR );

    DosCreateEventSem( NULL, &busy_ev, 0, FALSE );
    rec[0].hsemCur = (HSEM)busy_ev;
    rec[0].ulUser = 7;
    DosCreateMuxWaitSem( NULL, &busy_mux, 1, rec, DCMW_WAIT_ANY );
    busy_rc = 99;
    DosCreateThread( &tid, mux_waiter, 0, 0, STACK_SIZE );
    DosSleep( 200 );
    check( "DosCloseMuxWaitSem(a thread waits)", DosCloseMuxWaitSem( busy_mux ), ERROR_SEM_BUSY );
    DosPostEventSem( busy_ev );
    DosWaitThread( &tid, DCWW_WAIT );
    check( "-> the muxwait waiter was woken", busy_rc, NO_ERROR );
    check( "DosCloseMuxWaitSem(nobody waits)", DosCloseMuxWaitSem( busy_mux ), NO_ERROR );
    DosCloseEventSem( busy_ev );
}

/* ---------- pipe semaphores ---------- */

static void pipe_semaphores( void )
{
    HPIPE        hs;
    HFILE        hc;
    HEV          hev;
    ULONG        action, n, rc, tries;
    PIPESEMSTATE st[4];

    check( "DosCreateNPipe", DosCreateNPipe( PIPE_NAME, &hs, NP_ACCESS_DUPLEX,
           NP_NOWAIT | NP_TYPE_BYTE | NP_READMODE_BYTE | 1, 4096, 2048, 0 ), NO_ERROR );
    DosConnectNPipe( hs );
    check( "DosOpen(pipe)", DosOpen( PIPE_NAME, &hc, &action, 0, 0, OPEN_ACTION_OPEN_IF_EXISTS,
           OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, 0 ), NO_ERROR );
    for( tries = 0; tries < 100 && DosConnectNPipe( hs ) != NO_ERROR; tries++ ) DosSleep( 10 );
    check( "DosConnectNPipe(connected)", DosConnectNPipe( hs ), NO_ERROR );

    DosCreateEventSem( NULL, &hev, DC_SEM_SHARED, FALSE );
    check( "DosSetNPipeSem(not a semaphore)", DosSetNPipeSem( hs, (HSEM)0xBAD0, 7 ), ERROR_SEM_NOT_FOUND );
    check( "DosSetNPipeSem", DosSetNPipeSem( hs, (HSEM)hev, 7 ), NO_ERROR );

    memset( st, 0xEE, sizeof( st ) );
    check( "DosQueryNPipeSemState(idle)", DosQueryNPipeSemState( (HSEM)hev, st, sizeof( st ) ), NO_ERROR );
    check( "-> write space", st[0].fStatus, NPSS_WSPACE );
    check( "-> key", st[0].usKey, 7 );
    check( "-> end of records", st[1].fStatus, NPSS_EOI );

    check( "DosWrite(client)", DosWrite( hc, "hello", 5, &n ), NO_ERROR );
    check( "DosWaitEventSem(pipe semaphore, data)", DosWaitEventSem( hev, 5000 ), NO_ERROR );
    check( "DosQueryNPipeSemState(data)", DosQueryNPipeSemState( (HSEM)hev, st, sizeof( st ) ), NO_ERROR );
    check( "-> data to read", st[0].fStatus, NPSS_RDATA );
    check( "-> bytes to read", st[0].usAvail, 5 );
    check( "-> key", st[0].usKey, 7 );
    check( "DosQueryNPipeSemState(small buffer)", DosQueryNPipeSemState( (HSEM)hev, st, 6 ),
           ERROR_BUFFER_OVERFLOW );

    DosResetEventSem( hev, &n );
    DosClose( hc );
    check( "DosWaitEventSem(pipe semaphore, close)", DosWaitEventSem( hev, 5000 ), NO_ERROR );
    rc = DosQueryNPipeSemState( (HSEM)hev, st, sizeof( st ) );
    check( "DosQueryNPipeSemState(closed)", rc, NO_ERROR );
    check( "-> unread data still reported", st[0].fStatus, NPSS_RDATA );
    check( "-> closed", st[1].fStatus, NPSS_CLOSE );
    check( "-> end of records", st[2].fStatus, NPSS_EOI );

    check( "DosSetNPipeSem(detach)", DosSetNPipeSem( hs, 0, 0 ), NO_ERROR );
    DosQueryNPipeSemState( (HSEM)hev, st, sizeof( st ) );
    check( "-> no records", st[0].fStatus, NPSS_EOI );
    DosClose( hs );
    DosCloseEventSem( hev );
}

/* ---------- suballocation ---------- */

static void suballoc_freed( void )
{
    PVOID base, again, block;

    check( "DosAllocMem", DosAllocMem( &base, 8192, PAG_READ | PAG_WRITE | PAG_COMMIT ), NO_ERROR );
    check( "DosSubSetMem", DosSubSetMem( base, DOSSUB_INIT, 8192 ), NO_ERROR );
    check( "DosSubAllocMem", DosSubAllocMem( base, &block, 64 ), NO_ERROR );
    check( "DosFreeMem(without DosSubUnsetMem)", DosFreeMem( base ), NO_ERROR );
    check( "DosAllocMem(again)", DosAllocMem( &again, 8192, PAG_READ | PAG_WRITE | PAG_COMMIT ), NO_ERROR );
    check( "DosSubSetMem(new pool)", DosSubSetMem( again, DOSSUB_INIT, 8192 ), NO_ERROR );
    check( "DosSubAllocMem(new pool)", DosSubAllocMem( again, &block, 64 ), NO_ERROR );
    check( "DosSubUnsetMem", DosSubUnsetMem( again ), NO_ERROR );
    DosFreeMem( again );
}

int main( void )
{
    event_flags();
    owner_died();
    busy_close();
    pipe_semaphores();
    suballoc_freed();
    done();
    return n_passed == n_checks ? 0 : 1;
}
