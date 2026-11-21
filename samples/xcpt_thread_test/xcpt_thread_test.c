/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * xcpt_thread_test.c - every thread has its own exception-handler chain
 *
 * Thread 1 registers handler A, a second thread registers handler B, and
 * each raises an exception with DosRaiseException while the other's
 * handler is registered. Checks:
 *   - a created thread starts with an empty chain (B's prev_structure is
 *     the chain end, not A),
 *   - an exception on thread 1 reaches A only, one on thread 2 B only,
 *   - XCPT_CONTINUE_EXECUTION returns from DosRaiseException with NO_ERROR
 *     and the caller's registers intact,
 *   - DosUnsetExceptionHandler on thread 2 leaves thread 1's chain alone,
 *   - the handler's CONTEXTRECORD has the raising thread's CS, SS and a
 *     stack pointer on its stack, at the offsets <bsexcpt.h> gives them.
 *
 * Each check is written to stdout:
 *
 *   xcpt_thread_test: <name>: ok
 *   xcpt_thread_test: <name>: FAIL got <value> want <value>
 *
 * followed by "xcpt_thread_test: done <passed>/<total>".
 */
#define INCL_DOSPROCESS
#define INCL_DOSSEMAPHORES
#define INCL_DOSEXCEPTIONS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define STACK_SIZE   0x4000
#define TEST_XCPT    0xE0001234UL      /* an application exception code */
#define CHAIN_END    ((PEXCEPTIONREGISTRATIONRECORD)0xFFFFFFFFUL)

static int n_checks = 0;
static int n_passed = 0;

static volatile ULONG hits_a;
static volatile ULONG hits_b;
static volatile ULONG b_prev;          /* B's prev_structure after registering */
static volatile ULONG b_raise_rc;
static volatile ULONG b_kept;          /* a value thread 2 keeps across the raise */
static HEV ready, go;
static volatile ULONG a_cs, a_ss, a_esp, a_flags;   /* from A's first CONTEXTRECORD */

#ifdef __WATCOMC__
static ULONG read_cs( void );
#pragma aux read_cs = "xor eax, eax" "mov ax, cs" value [eax];
static ULONG read_ss( void );
#pragma aux read_ss = "xor eax, eax" "mov ax, ss" value [eax];
#else
static ULONG read_cs( void ) { ULONG v = 0; __asm__ volatile( "mov %%cs, %w0" : "+r"( v ) ); return v; }
static ULONG read_ss( void ) { ULONG v = 0; __asm__ volatile( "mov %%ss, %w0" : "+r"( v ) ); return v; }
#endif

/* ---------- output (no C library stdio) ---------- */

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
    char *p = append_str( line, "xcpt_thread_test: " );
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

/* ---------- handlers ---------- */

static ULONG APIENTRY handler_a( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                                 PCONTEXTRECORD ctx, PVOID unused )
{
    if( rep->ExceptionNum == TEST_XCPT ) {
        if( hits_a++ == 0 ) {
            a_cs = ctx->ctx_SegCs;
            a_ss = ctx->ctx_SegSs;
            a_esp = ctx->ctx_RegEsp;
            a_flags = ctx->ContextFlags;
        }
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

static ULONG APIENTRY handler_b( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                                 PCONTEXTRECORD ctx, PVOID unused )
{
    if( rep->ExceptionNum == TEST_XCPT ) {
        hits_b++;
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

static APIRET raise_test( void )
{
    EXCEPTIONREPORTRECORD rep;
    memset( &rep, 0, sizeof( rep ) );
    rep.ExceptionNum = TEST_XCPT;
    return DosRaiseException( &rep );
}

/* ---------- thread 2 ---------- */

static void APIENTRY second( ULONG arg )
{
    EXCEPTIONREGISTRATIONRECORD rec_b;
    ULONG posts;
    volatile ULONG kept = arg;

    memset( &rec_b, 0, sizeof( rec_b ) );
    rec_b.ExceptionHandler = handler_b;
    DosSetExceptionHandler( &rec_b );
    b_prev = (ULONG)rec_b.prev_structure;
    DosPostEventSem( ready );
    DosWaitEventSem( go, SEM_INDEFINITE_WAIT );
    DosResetEventSem( go, &posts );

    b_raise_rc = raise_test();
    b_kept = kept;
    DosUnsetExceptionHandler( &rec_b );
    DosExit( EXIT_THREAD, 0 );
}

/* ---------- thread 1 ---------- */

int main( void )
{
    EXCEPTIONREGISTRATIONRECORD rec_a;
    TID   tid;
    ULONG posts;
    APIRET rc;
    volatile ULONG kept = 0x5A5A1234UL;
    char  line[64];
    char *p;

    DosCreateEventSem( NULL, &ready, 0, FALSE );
    DosCreateEventSem( NULL, &go, 0, FALSE );

    memset( &rec_a, 0, sizeof( rec_a ) );
    rec_a.ExceptionHandler = handler_a;
    check( "DosSetExceptionHandler(A)", DosSetExceptionHandler( &rec_a ), NO_ERROR );

    rc = raise_test();
    check( "raise on thread 1 returns NO_ERROR", rc, NO_ERROR );
    check( "A handled thread 1's exception", hits_a, 1 );
    check( "thread 1's local kept across the raise", kept == 0x5A5A1234UL, TRUE );
    check( "the context has the integer, control and segment registers", a_flags & 7, 7 );
    check( "the context's CS is the program's", a_cs, read_cs() );
    check( "the context's SS is the program's", a_ss, read_ss() );
    check( "the context's ESP is on thread 1's stack",
           a_esp != 0 && a_esp < (ULONG)&kept && (ULONG)&kept - a_esp < 0x10000, TRUE );

    check( "DosCreateThread", DosCreateThread( &tid, second, 0x7777, 0, STACK_SIZE ), NO_ERROR );
    DosWaitEventSem( ready, SEM_INDEFINITE_WAIT );
    DosResetEventSem( ready, &posts );
    check( "thread 2 starts with an empty chain", b_prev == (ULONG)CHAIN_END, TRUE );

    /* B is registered on thread 2 now: thread 1's exception must reach A. */
    rc = raise_test();
    check( "raise on thread 1 while B is registered", rc, NO_ERROR );
    check( "A handled it", hits_a, 2 );
    check( "B did not see thread 1's exception", hits_b, 0 );

    DosPostEventSem( go );
    check( "DosWaitThread", DosWaitThread( &tid, DCWW_WAIT ), NO_ERROR );
    check( "B handled thread 2's exception", hits_b, 1 );
    check( "A did not see thread 2's exception", hits_a, 2 );
    check( "raise on thread 2 returns NO_ERROR", b_raise_rc, NO_ERROR );
    check( "thread 2's local kept across the raise", b_kept == 0x7777, TRUE );

    /* Thread 2's unset left thread 1's chain alone: A still handles. */
    rc = raise_test();
    check( "raise on thread 1 after thread 2 ended", rc, NO_ERROR );
    check( "A still handles", hits_a, 3 );
    check( "DosUnsetExceptionHandler(A)", DosUnsetExceptionHandler( &rec_a ), NO_ERROR );

    p = append_str( line, "xcpt_thread_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );
    return n_passed == n_checks ? 0 : 1;
}
