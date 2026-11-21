/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * exitlist_test.c - DosExitList: routines run when the process ends
 *
 * Built five times from this source (see the Makefile); each registers the
 * same exit list and ends the process another way:
 *
 *   exitlist_test.exe     thread 1 calls DosExit(EXIT_PROCESS, 3)   -> code 3
 *   exitlist_thread.exe   thread 2 calls DosExit(EXIT_PROCESS, 4)
 *                         while thread 1 waits                     -> code 4
 *   exitlist_mainlast.exe thread 2 ends first, then thread 1 ends with
 *                         DosExit(EXIT_THREAD, 6), the last thread -> code 6
 *   exitlist_nomain.exe   thread 1 ends first with DosExit(EXIT_THREAD);
 *                         thread 2 then ends the process with
 *                         DosExit(EXIT_THREAD, 5). Thread 1 has gone, so
 *                         the exit list does not run (Control Program
 *                         Guide)                                    -> code 5
 *   exitlist_hlt.exe      thread 1 executes HLT: a trap; the routines
 *                         get TC_TRAP                               -> code 1
 *
 * Checked before the process ends: DosExitList's error codes
 * (ERROR_INVALID_DATA for a non-zero high word, an order with
 * EXLST_REMOVE / EXLST_EXIT, removing a routine not on the list or adding
 * NULL; ERROR_INVALID_FUNCTION for an unknown action and for EXLST_EXIT
 * outside exit-list processing).
 *
 * Checked by the routines: they run on thread 1, lowest order first, the
 * last added first among equal orders; each gets TC_EXIT (TC_TRAP after
 * HLT); a routine may add (order
 * 0x30, runs last) and remove (a routine not yet run) others; semaphores
 * still work; the last routine prints the sequence it saw. Each routine
 * ends with DosExitList(EXLST_EXIT, 0).
 *
 * Output, one line per check:
 *
 *   exitlist_test: <name>: ok
 *   exitlist_test: <name>: FAIL got <value> want <value>
 *
 * then "exitlist_test: done <passed>/<total>" (before the process ends)
 * and "exitlist_test: exit list done <passed>/<total>" from the last routine.
 */
#define INCL_DOSPROCESS
#define INCL_DOSSEMAPHORES
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define STACK_SIZE  0x4000

/* The termination code the routines get. */
#if defined( HLT_EXIT )
#define EXPECTED_TC TC_TRAP
#else
#define EXPECTED_TC TC_EXIT
#endif

static int n_checks = 0;
static int n_passed = 0;
static char seq[16];
static int  n_seq = 0;
static HEV  hev;

/* ---------- output (no C library stdio: it may be gone at exit) ---------- */

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
    char *p = append_str( line, "exitlist_test: " );
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

static void done( const char *what )
{
    char  line[80];
    char *p = append_str( line, "exitlist_test: " );
    p = append_str( p, what );
    p = append_str( p, " " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );
}

#define ORDER( n )  ( (ULONG)(n) << 8 )

/* ---------- the exit-list routines ---------- */

static VOID APIENTRY routine_never( ULONG tc );
static VOID APIENTRY routine_last( ULONG tc );

static void ran( char name, ULONG tc )
{
    char  line[64];
    char *p = append_str( line, "exitlist_test: routine " );
    *p++ = name;
    p = append_str( p, " tc=" );
    p = append_long( p, tc );
    append_str( p, "\r\n" );
    write_stdout( line );
    if( n_seq < (int)sizeof( seq ) - 1 ) seq[n_seq++] = name;
}

/* Order 0x10: first. Adds routine_last, removes routine_never. */
static VOID APIENTRY routine_b( ULONG tc )
{
    PTIB ptib;
    PPIB ppib;
    ran( 'B', tc );
    check( "B: termination code", tc, EXPECTED_TC );
    DosGetInfoBlocks( &ptib, &ppib );
    check( "B: runs on thread 1", ptib->tib_ptib2->tib2_ultid, 1 );
    check( "B: add a routine while the list runs",
           DosExitList( EXLST_ADD | ORDER( 0x30 ), (PFNEXITLIST)routine_last ), NO_ERROR );
    check( "B: remove a routine not yet run",
           DosExitList( EXLST_REMOVE, (PFNEXITLIST)routine_never ), NO_ERROR );
    check( "B: semaphores still work", DosPostEventSem( hev ), NO_ERROR );
    DosExitList( EXLST_EXIT, 0 );
}

/* Order 0x20, added first: runs after routine_c. */
static VOID APIENTRY routine_a( ULONG tc )
{
    ran( 'A', tc );
    DosExitList( EXLST_EXIT, 0 );
}

/* Order 0x20, added second: runs before routine_a. */
static VOID APIENTRY routine_c( ULONG tc )
{
    ran( 'C', tc );
    DosExitList( EXLST_EXIT, 0 );
}

/* Order 0x28: removed by routine_b, never runs. */
static VOID APIENTRY routine_never( ULONG tc )
{
    ran( 'N', tc );
    DosExitList( EXLST_EXIT, 0 );
}

/* Order 0x30, added by routine_b: the last one. */
static VOID APIENTRY routine_last( ULONG tc )
{
    char  line[48];
    ULONG count = 0;
    ran( 'L', tc );
    seq[n_seq] = '\0';
    check( "L: routines in order (B C A L)", strcmp( seq, "BCAL" ), 0 );
    DosQueryEventSem( hev, &count );
    check( "L: B's post seen", count, 1 );
    append_str( append_str( line, "exitlist_test: sequence " ), seq );
    append_str( line + strlen( line ), "\r\n" );
    write_stdout( line );
    done( "exit list done" );
    DosExitList( EXLST_EXIT, 0 );
}

/* ---------- setup ---------- */

static void register_routines( void )
{
    check( "DosExitList(non-zero high word)",
           DosExitList( 0x10000 | EXLST_ADD, (PFNEXITLIST)routine_a ), ERROR_INVALID_DATA );
    check( "DosExitList(unknown action)",
           DosExitList( 4, (PFNEXITLIST)routine_a ), ERROR_INVALID_FUNCTION );
    check( "DosExitList(EXLST_REMOVE with an order)",
           DosExitList( EXLST_REMOVE | ORDER( 1 ), (PFNEXITLIST)routine_a ), ERROR_INVALID_DATA );
    check( "DosExitList(EXLST_EXIT outside exit-list processing)",
           DosExitList( EXLST_EXIT, 0 ), ERROR_INVALID_FUNCTION );
    check( "DosExitList(EXLST_REMOVE, routine not on the list)",
           DosExitList( EXLST_REMOVE, (PFNEXITLIST)routine_a ), ERROR_INVALID_DATA );
    check( "DosExitList(EXLST_ADD, NULL)", DosExitList( EXLST_ADD, NULL ), ERROR_INVALID_DATA );

    check( "add A (order 0x20)", DosExitList( EXLST_ADD | ORDER( 0x20 ), (PFNEXITLIST)routine_a ), NO_ERROR );
    check( "add B (order 0x10)", DosExitList( EXLST_ADD | ORDER( 0x10 ), (PFNEXITLIST)routine_b ), NO_ERROR );
    check( "add C (order 0x20)", DosExitList( EXLST_ADD | ORDER( 0x20 ), (PFNEXITLIST)routine_c ), NO_ERROR );
    check( "add N (order 0x28)", DosExitList( EXLST_ADD | ORDER( 0x28 ), (PFNEXITLIST)routine_never ), NO_ERROR );
    check( "add and remove L", DosExitList( EXLST_ADD | ORDER( 0x05 ), (PFNEXITLIST)routine_last ), NO_ERROR );
    check( "-> removed", DosExitList( EXLST_REMOVE, (PFNEXITLIST)routine_last ), NO_ERROR );
    check( "DosCreateEventSem", DosCreateEventSem( NULL, &hev, 0, FALSE ), NO_ERROR );
}

#if defined( THREAD_EXIT ) || defined( MAIN_LAST ) || defined( NO_MAIN )
static VOID APIENTRY second_thread( ULONG arg )
{
    (void)arg;
#if defined( THREAD_EXIT )
    write_stdout( "exitlist_test: thread 2 ends the process\r\n" );
    DosExit( EXIT_PROCESS, 4 );
#elif defined( NO_MAIN )
    DosSleep( 200 );
    write_stdout( "exitlist_test: thread 2 ends the process\r\n" );
    DosExit( EXIT_THREAD, 5 );
#else
    DosExit( EXIT_THREAD, 0 );
#endif
}
#endif

int main( void )
{
#if defined( THREAD_EXIT ) || defined( MAIN_LAST ) || defined( NO_MAIN )
    TID   tid;
    HEV   never;
#endif
    register_routines();
#if defined( THREAD_EXIT )
    DosCreateEventSem( NULL, &never, 0, FALSE );
    check( "DosCreateThread", DosCreateThread( &tid, second_thread, 0, 0, STACK_SIZE ), NO_ERROR );
    done( "done" );
    DosWaitEventSem( never, SEM_INDEFINITE_WAIT );
    write_stdout( "exitlist_test: thread 1 woke up: FAIL\r\n" );
    return 1;
#elif defined( MAIN_LAST )
    (void)never;
    check( "DosCreateThread", DosCreateThread( &tid, second_thread, 0, 0, STACK_SIZE ), NO_ERROR );
    check( "DosWaitThread", DosWaitThread( &tid, DCWW_WAIT ), NO_ERROR );
    done( "done" );
    write_stdout( "exitlist_test: thread 1 ends, the last thread\r\n" );
    DosExit( EXIT_THREAD, 6 );
    return 1;
#elif defined( NO_MAIN )
    (void)never;
    check( "DosCreateThread", DosCreateThread( &tid, second_thread, 0, 0, STACK_SIZE ), NO_ERROR );
    done( "done" );
    write_stdout( "exitlist_test: thread 1 ends first\r\n" );
    DosExit( EXIT_THREAD, 0 );
    return 1;
#elif defined( HLT_EXIT )
    done( "done" );
    write_stdout( "exitlist_test: thread 1 executes HLT\r\n" );
#ifdef __WATCOMC__
    _asm hlt;
#else
    __asm__ volatile( "hlt" );
#endif
    write_stdout( "exitlist_test: went on after HLT: FAIL\r\n" );
    return 1;
#else
    done( "done" );
    DosExit( EXIT_PROCESS, 3 );
    return 1;
#endif
}
