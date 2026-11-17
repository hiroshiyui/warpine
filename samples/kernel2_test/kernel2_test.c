/*
 * kernel2_test.c - OS/2 2.0 kernel calls, second batch:
 *
 *   DosEnterCritSec / DosExitCritSec      other threads stop, nesting,
 *                                         ERROR_CRITSEC_UNDERFLOW, a thread
 *                                         ending inside a section ends it
 *   DosAsyncTimer / DosStartTimer /       one and repeated posts of a shared
 *   DosStopTimer                          event semaphore, ERROR_TS_SEMHANDLE,
 *                                         ERROR_TS_HANDLE
 *   DosSearchPath                         path order, SEARCH_CUR_DIRECTORY,
 *                                         SEARCH_ENVIRONMENT, wildcards,
 *                                         ERROR_BUFFER_OVERFLOW
 *   DosQueryModuleName                    the executable, a DLL,
 *                                         ERROR_BAD_LENGTH, bad handle
 *   DosSubSetMem / DosSubAllocMem /       pools, 8-byte rounding, grow,
 *   DosSubFreeMem / DosSubUnsetMem        ERROR_DOSSUB_* codes
 *
 * Each check is written to stdout:
 *
 *   kernel2_test: <name>: ok
 *   kernel2_test: <name>: FAIL got <value> want <value>
 *
 * followed by "kernel2_test: done <passed>/<total>".
 */
#define INCL_DOSPROCESS
#define INCL_DOSSEMAPHORES
#define INCL_DOSDATETIME
#define INCL_DOSMISC
#define INCL_DOSMODULEMGR
#define INCL_DOSMEMMGR
#define INCL_DOSFILEMGR
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define STACK_SIZE  0x4000

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
    char  line[200];
    char *p = append_str( line, "kernel2_test: " );
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

static void note( const char *what, const char *text )
{
    char  line[400];
    char *p = append_str( line, "kernel2_test: note " );
    p = append_str( p, what );
    p = append_str( p, " = " );
    p = append_str( p, text );
    append_str( p, "\r\n" );
    write_stdout( line );
}

static void done( void )
{
    char  line[64];
    char *p = append_str( line, "kernel2_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );
}

/* Whether s ends with tail, ignoring case. */
static int ends_with( const char *s, const char *tail )
{
    size_t ls = strlen( s ), lt = strlen( tail ), i;
    if( lt > ls ) return 0;
    for( i = 0; i < lt; i++ ) {
        char a = s[ls - lt + i], b = tail[i];
        if( a >= 'a' && a <= 'z' ) a = (char)( a - 'a' + 'A' );
        if( b >= 'a' && b <= 'z' ) b = (char)( b - 'a' + 'A' );
        if( a != b ) return 0;
    }
    return 1;
}

static ULONG ms_now( void )
{
    ULONG ms = 0;
    DosQuerySysInfo( QSV_MS_COUNT, QSV_MS_COUNT, &ms, sizeof( ms ) );
    return ms;
}

/* ---------- critical sections ---------- */

static volatile ULONG counter = 0;
static volatile ULONG stop_counting = 0;

static VOID APIENTRY count_thread( ULONG arg )
{
    (void)arg;
    while( !stop_counting ) {
        counter++;
        DosSleep( 1 );
    }
}

static VOID APIENTRY end_in_section_thread( ULONG arg )
{
    (void)arg;
    DosEnterCritSec();
    DosEnterCritSec();
    DosExit( EXIT_THREAD, 0 );
}

static void crit_sections( void )
{
    TID   tid, tid2;
    ULONG c1, c2;

    check( "DosExitCritSec(not in one)", DosExitCritSec(), ERROR_CRITSEC_UNDERFLOW );
    check( "DosCreateThread(counter)", DosCreateThread( &tid, count_thread, 0, 0, STACK_SIZE ), NO_ERROR );
    DosSleep( 100 );
    check( "-> the counter runs", counter > 0, 1 );

    check( "DosEnterCritSec", DosEnterCritSec(), NO_ERROR );
    DosSleep( 50 );
    c1 = counter;
    DosSleep( 200 );
    c2 = counter;
    check( "-> the other thread stops", c2 - c1, 0 );
    check( "DosEnterCritSec(nested)", DosEnterCritSec(), NO_ERROR );
    check( "DosExitCritSec(inner)", DosExitCritSec(), NO_ERROR );
    DosSleep( 150 );
    check( "-> still stopped while nested", counter - c2, 0 );
    check( "DosExitCritSec(outer)", DosExitCritSec(), NO_ERROR );
    DosSleep( 150 );
    check( "-> the other thread runs again", counter > c2, 1 );
    check( "DosExitCritSec(after the end)", DosExitCritSec(), ERROR_CRITSEC_UNDERFLOW );

    stop_counting = 1;
    check( "DosWaitThread(counter)", DosWaitThread( &tid, DCWW_WAIT ), NO_ERROR );

    /* A thread that ends inside a section ends the section with it. */
    check( "DosCreateThread(ends inside)", DosCreateThread( &tid2, end_in_section_thread, 0, 0, STACK_SIZE ), NO_ERROR );
    check( "DosWaitThread(ends inside)", DosWaitThread( &tid2, DCWW_WAIT ), NO_ERROR );
    check( "-> its section ended", DosExitCritSec(), ERROR_CRITSEC_UNDERFLOW );
}

/* ---------- timers ---------- */

static void timers( void )
{
    HEV    hev, private_hev;
    HTIMER ht = 0, ht2 = 0;
    ULONG  count, t0, tick = 0, i;
    APIRET rc;

    DosQuerySysInfo( QSV_TIMER_INTERVAL, QSV_TIMER_INTERVAL, &tick, sizeof( tick ) );
    check( "QSV_TIMER_INTERVAL in tenths of a ms (1..100 ms)", tick >= 10 && tick <= 1000, 1 );

    check( "DosCreateEventSem(shared)", DosCreateEventSem( NULL, &hev, DC_SEM_SHARED, FALSE ), NO_ERROR );
    check( "DosCreateEventSem(private)", DosCreateEventSem( NULL, &private_hev, 0, FALSE ), NO_ERROR );
    check( "DosAsyncTimer(private sem)", DosAsyncTimer( 10, (HSEM)private_hev, &ht ), ERROR_TS_SEMHANDLE );
    check( "DosStartTimer(private sem)", DosStartTimer( 10, (HSEM)private_hev, &ht ), ERROR_TS_SEMHANDLE );

    t0 = ms_now();
    check( "DosAsyncTimer(200 ms)", DosAsyncTimer( 200, (HSEM)hev, &ht ), NO_ERROR );
    check( "-> a timer handle", ht != 0, 1 );
    check( "-> not posted at once", DosWaitEventSem( hev, SEM_IMMEDIATE_RETURN ), ERROR_TIMEOUT );
    check( "DosWaitEventSem(timer)", DosWaitEventSem( hev, 5000 ), NO_ERROR );
    check( "-> at least 200 ms later", ms_now() - t0 >= 190, 1 );
    DosSleep( 300 );
    check( "DosResetEventSem", DosResetEventSem( hev, &count ), NO_ERROR );
    check( "-> posted once", count, 1 );
    check( "DosStopTimer(expired one-shot)", DosStopTimer( ht ), ERROR_TS_HANDLE );

    check( "DosStartTimer(50 ms)", DosStartTimer( 50, (HSEM)hev, &ht ), NO_ERROR );
    rc = NO_ERROR;
    for( i = 0; i < 3 && rc == NO_ERROR; i++ ) {
        rc = DosWaitEventSem( hev, 5000 );
        if( rc == NO_ERROR ) rc = DosResetEventSem( hev, &count );
    }
    check( "-> posted three times", rc, NO_ERROR );
    check( "DosStopTimer(periodic)", DosStopTimer( ht ), NO_ERROR );
    DosSleep( 100 );
    DosResetEventSem( hev, &count );
    DosSleep( 300 );
    check( "-> no post after the stop", DosQueryEventSem( hev, &count ) == NO_ERROR && count == 0, 1 );
    check( "DosStopTimer(stopped)", DosStopTimer( ht ), ERROR_TS_HANDLE );

    check( "DosAsyncTimer(10 s)", DosAsyncTimer( 10000, (HSEM)hev, &ht2 ), NO_ERROR );
    check( "DosStopTimer(before it fires)", DosStopTimer( ht2 ), NO_ERROR );
    DosSleep( 100 );
    check( "-> never posted", DosWaitEventSem( hev, SEM_IMMEDIATE_RETURN ), ERROR_TIMEOUT );
    check( "DosStopTimer(bad handle)", DosStopTimer( 0xBADBAD ), ERROR_TS_HANDLE );

    DosCloseEventSem( hev );
    DosCloseEventSem( private_hev );
}

/* ---------- DosSearchPath / DosQueryModuleName ---------- */

static void search_path( void )
{
    char   buf[300];
    HFILE  hf;
    ULONG  action;
    APIRET rc;

    DosDelete( "K2DIR\\K2FILE.TXT" );
    DosDeleteDir( "K2DIR" );
    check( "DosCreateDir(K2DIR)", DosCreateDir( "K2DIR", NULL ), NO_ERROR );
    rc = DosOpen( "K2DIR\\K2FILE.TXT", &hf, &action, 0, FILE_NORMAL,
                  OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                  OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYNONE, NULL );
    check( "DosOpen(K2DIR\\K2FILE.TXT)", rc, NO_ERROR );
    if( rc == NO_ERROR ) DosClose( hf );

    buf[0] = '\0';
    check( "DosSearchPath(path)", DosSearchPath( SEARCH_PATH, "Q:\\NOPE;NOSUCHDIR;K2DIR",
                                                  "K2FILE.TXT", buf, sizeof( buf ) ), NO_ERROR );
    note( "found", buf );
    check( "-> fully qualified", buf[1] == ':' && buf[2] == '\\', 1 );
    check( "-> in K2DIR", ends_with( buf, "\\K2DIR\\K2FILE.TXT" ), 1 );
    rc = DosOpen( buf, &hf, &action, 0, FILE_NORMAL, OPEN_ACTION_FAIL_IF_NEW | OPEN_ACTION_OPEN_IF_EXISTS,
                  OPEN_ACCESS_READONLY | OPEN_SHARE_DENYNONE, NULL );
    check( "-> DosOpen opens it", rc, NO_ERROR );
    if( rc == NO_ERROR ) DosClose( hf );

    check( "DosSearchPath(not there)", DosSearchPath( SEARCH_PATH, "NOSUCHDIR", "K2FILE.TXT",
                                                       buf, sizeof( buf ) ), ERROR_FILE_NOT_FOUND );
    check( "DosSearchPath(wildcards)", DosSearchPath( SEARCH_PATH, "NOSUCHDIR;K2DIR", "K2*.TXT",
                                                       buf, sizeof( buf ) ), NO_ERROR );
    check( "-> keeps the wildcards", ends_with( buf, "\\K2DIR\\K2*.TXT" ), 1 );
    check( "DosSearchPath(buffer too small)", DosSearchPath( SEARCH_PATH, "K2DIR", "K2FILE.TXT",
                                                              buf, 10 ), ERROR_BUFFER_OVERFLOW );
    check( "DosSearchPath(reserved flag)", DosSearchPath( 8, "K2DIR", "K2FILE.TXT",
                                                           buf, sizeof( buf ) ), ERROR_INVALID_PARAMETER );
    check( "DosSearchPath(no such variable)", DosSearchPath( SEARCH_ENVIRONMENT, "K2_NO_SUCH_VAR",
                                                              "K2FILE.TXT", buf, sizeof( buf ) ),
           ERROR_ENVVAR_NOT_FOUND );

    check( "DosSetCurrentDir(K2DIR)", DosSetCurrentDir( "K2DIR" ), NO_ERROR );
    check( "DosSearchPath(no current dir)", DosSearchPath( SEARCH_PATH, "NOSUCHDIR", "K2FILE.TXT",
                                                            buf, sizeof( buf ) ), ERROR_FILE_NOT_FOUND );
    check( "DosSearchPath(SEARCH_CUR_DIRECTORY)", DosSearchPath( SEARCH_CUR_DIRECTORY, "NOSUCHDIR",
                                                                 "K2FILE.TXT", buf, sizeof( buf ) ), NO_ERROR );
    check( "-> in the current directory", ends_with( buf, "\\K2DIR\\K2FILE.TXT" ), 1 );
    check( "DosSearchPath(.)", DosSearchPath( SEARCH_PATH, ".", "K2FILE.TXT", buf, sizeof( buf ) ), NO_ERROR );
    check( "-> . is the current directory", ends_with( buf, "\\K2DIR\\K2FILE.TXT" ), 1 );
    check( "DosSetCurrentDir(..)", DosSetCurrentDir( ".." ), NO_ERROR );

    check( "DosDelete(K2FILE.TXT)", DosDelete( "K2DIR\\K2FILE.TXT" ), NO_ERROR );
    check( "DosDeleteDir(K2DIR)", DosDeleteDir( "K2DIR" ), NO_ERROR );
}

static void module_name( void )
{
    char    buf[300];
    PTIB    ptib;
    PPIB    ppib;
    HMODULE hmod = 0;

    DosGetInfoBlocks( &ptib, &ppib );
    buf[0] = '\0';
    check( "DosQueryModuleName(exe)", DosQueryModuleName( ppib->pib_hmte, sizeof( buf ), buf ), NO_ERROR );
    note( "exe", buf );
    check( "-> the executable", ends_with( buf, "\\KERNEL2_TEST.EXE" ) && buf[1] == ':', 1 );
    check( "DosQueryModuleName(short buffer)", DosQueryModuleName( ppib->pib_hmte, 4, buf ), ERROR_BAD_LENGTH );
    check( "DosQueryModuleName(bad handle)", DosQueryModuleName( 0xBADBAD, sizeof( buf ), buf ),
           ERROR_INVALID_HANDLE );
    check( "DosLoadModule(UCONV)", DosLoadModule( NULL, 0, "UCONV", &hmod ), NO_ERROR );
    check( "DosQueryModuleName(UCONV)", DosQueryModuleName( hmod, sizeof( buf ), buf ), NO_ERROR );
    note( "UCONV", buf );
    check( "-> UCONV.DLL", ends_with( buf, "\\UCONV.DLL" ) && buf[1] == ':', 1 );
    DosFreeModule( hmod );
}

/* ---------- memory suballocation ---------- */

static void suballocation( void )
{
    PBYTE  mem = NULL;
    PVOID  a = NULL, b = NULL, c = NULL;
    APIRET rc;

    rc = DosAllocMem( (PPVOID)&mem, 16384, PAG_COMMIT | PAG_READ | PAG_WRITE );
    check( "DosAllocMem(16 KB)", rc, NO_ERROR );
    if( rc != NO_ERROR ) return;

    check( "DosSubSetMem(bad flag)", DosSubSetMem( mem, DOSSUB_INIT | 0x10, 4096 ), ERROR_DOSSUB_BADFLAG );
    check( "DosSubSetMem(71 bytes)", DosSubSetMem( mem, DOSSUB_INIT, 71 ), ERROR_INVALID_PARAMETER );
    check( "DosSubAllocMem(no pool)", DosSubAllocMem( mem, &a, 8 ), ERROR_DOSSUB_CORRUPTED );
    check( "DosSubSetMem(init, 4 KB)", DosSubSetMem( mem, DOSSUB_INIT | DOSSUB_SPARSE_OBJ, 4096 ), NO_ERROR );

    check( "DosSubAllocMem(57)", DosSubAllocMem( mem, &a, 57 ), NO_ERROR );
    check( "-> inside the pool", (PBYTE)a >= mem + 64 && (PBYTE)a < mem + 4096, 1 );
    check( "-> on 8 bytes", ( (PBYTE)a - mem ) % 8, 0 );
    check( "DosSubAllocMem(8)", DosSubAllocMem( mem, &b, 8 ), NO_ERROR );
    check( "-> 57 bytes took 64", (PBYTE)b == (PBYTE)a + 64 || (PBYTE)a == (PBYTE)b + 8, 1 );
    memset( a, 0xA5, 57 );
    memset( b, 0x5A, 8 );
    check( "-> blocks hold data", ( (PBYTE)a )[56] == 0xA5 && ( (PBYTE)b )[7] == 0x5A, 1 );

    check( "DosSubAllocMem(0)", DosSubAllocMem( mem, &c, 0 ), ERROR_INVALID_PARAMETER );
    check( "DosSubAllocMem(more than the pool)", DosSubAllocMem( mem, &c, 4096 ), ERROR_DOSSUB_BADSIZE );
    check( "DosSubAllocMem(no room)", DosSubAllocMem( mem, &c, 4096 - 64 ), ERROR_DOSSUB_NOMEM );

    check( "DosSubFreeMem(b)", DosSubFreeMem( mem, b, 8 ), NO_ERROR );
    check( "DosSubFreeMem(b again)", DosSubFreeMem( mem, b, 8 ), ERROR_DOSSUB_OVERLAP );
    check( "DosSubFreeMem(a)", DosSubFreeMem( mem, a, 57 ), NO_ERROR );
    check( "DosSubAllocMem(whole pool)", DosSubAllocMem( mem, &c, 4096 - 64 ), NO_ERROR );
    check( "DosSubFreeMem(whole pool)", DosSubFreeMem( mem, c, 4096 - 64 ), NO_ERROR );

    check( "DosSubSetMem(grow, other sparse flag)", DosSubSetMem( mem, DOSSUB_GROW, 8192 ), ERROR_DOSSUB_BADFLAG );
    check( "DosSubSetMem(grow smaller)", DosSubSetMem( mem, DOSSUB_GROW | DOSSUB_SPARSE_OBJ, 2048 ), ERROR_DOSSUB_SHRINK );
    check( "DosSubSetMem(grow to 8 KB)", DosSubSetMem( mem, DOSSUB_GROW | DOSSUB_SPARSE_OBJ, 8192 ), NO_ERROR );
    check( "DosSubAllocMem(6 KB)", DosSubAllocMem( mem, &c, 6144 ), NO_ERROR );
    memset( c, 0x33, 6144 );
    check( "DosSubFreeMem(6 KB)", DosSubFreeMem( mem, c, 6144 ), NO_ERROR );

    check( "DosSubUnsetMem", DosSubUnsetMem( mem ), NO_ERROR );
    check( "DosSubUnsetMem(again)", DosSubUnsetMem( mem ), ERROR_DOSSUB_CORRUPTED );
    check( "DosSubAllocMem(after unset)", DosSubAllocMem( mem, &a, 8 ), ERROR_DOSSUB_CORRUPTED );

    check( "DosSubSetMem(serialized)", DosSubSetMem( mem, DOSSUB_INIT | DOSSUB_SERIALIZE, 16384 ), NO_ERROR );
    check( "DosSubAllocMem(serialized)", DosSubAllocMem( mem, &a, 100 ), NO_ERROR );
    check( "DosSubUnsetMem(serialized)", DosSubUnsetMem( mem ), NO_ERROR );
    check( "DosFreeMem", DosFreeMem( mem ), NO_ERROR );
}

int main( void )
{
    crit_sections();
    timers();
    search_path();
    module_name();
    suballocation();
    done();
    return n_passed == n_checks ? 0 : 1;
}
