/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * xcpt_unwind_test.c - DosUnwindException, longjmp and handlers that leave
 *
 * Registration records live in nested functions' frames: O (outer), M
 * (middle), I (inner). Checks:
 *   - DosUnwindException( &O, NULL, NULL ) calls I, then M, each with
 *     EH_UNWINDING set, removes them, leaves O at the head and returns
 *     NO_ERROR; the report record it makes says XCPT_UNWIND;
 *   - a report record given is the one the handlers get, its flags with
 *     EH_UNWINDING added;
 *   - longjmp unwinds the handlers registered since setjmp (Open Watcom's
 *     longjmp calls DosUnwindException with a target address after the
 *     call; the SDK's does the same);
 *   - a handler that leaves an access violation by longjmp, three times
 *     over, and the exceptions after it still work (DosRaiseException
 *     with a handler continuing returns NO_ERROR);
 *   - DosRaiseException fills in ExceptionAddress: an address in the
 *     function that raised;
 *   - on a thread of its own, END_OF_CHAIN and UNWIND_ALL (0) unwind the
 *     whole chain, which ends empty;
 *   - a target record that is not on the chain is refused, and nothing is
 *     unwound.
 *
 * Each check is written to stdout:
 *
 *   xcpt_unwind_test: <name>: ok
 *   xcpt_unwind_test: <name>: FAIL got <value> want <value>
 *
 * followed by "xcpt_unwind_test: done <passed>/<total>".
 */
#define INCL_DOSPROCESS
#define INCL_DOSSEMAPHORES
#define INCL_DOSEXCEPTIONS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include <setjmp.h>

#define TEST_XCPT    0xE0005678UL      /* an application exception code */
#define CHAIN_END    ((PEXCEPTIONREGISTRATIONRECORD)0xFFFFFFFFUL)
#define STACK_SIZE   0x8000
#ifndef XCPT_UNWIND
#define XCPT_UNWIND  0xC0000026UL
#endif

static int n_checks = 0;
static int n_passed = 0;

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
    char  line[200];
    char *p = append_str( line, "xcpt_unwind_test: " );
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

static void check_str( const char *what, const char *got, const char *want )
{
    char  line[200];
    char *p = append_str( line, "xcpt_unwind_test: " );
    p = append_str( p, what );
    if( strcmp( got, want ) == 0 ) {
        append_str( p, ": ok\r\n" );
        n_passed++;
    } else {
        p = append_str( p, ": FAIL got \"" );
        p = append_str( p, got );
        p = append_str( p, "\" want \"" );
        p = append_str( p, want );
        append_str( p, "\"\r\n" );
    }
    write_stdout( line );
    n_checks++;
}

/* ---------- the calls the handlers see ---------- */

static char  trace[64];                /* one letter per call: I, M, O, ... */
static int   n_trace;
static ULONG all_unwinding;            /* every unwinding call had EH_UNWINDING */
static ULONG last_num;                 /* the last report's ExceptionNum */
static PEXCEPTIONREPORTRECORD last_rep;

static void note( char who, PEXCEPTIONREPORTRECORD rep )
{
    if( n_trace < (int)sizeof( trace ) - 1 ) {
        trace[n_trace++] = who;
        trace[n_trace] = '\0';
    }
    if( !( rep->fHandlerFlags & EH_UNWINDING ) ) all_unwinding = 0;
    last_num = rep->ExceptionNum;
    last_rep = rep;
}

static void reset_trace( void )
{
    n_trace = 0;
    trace[0] = '\0';
    all_unwinding = 1;
    last_num = 0;
    last_rep = NULL;
}

static PEXCEPTIONREGISTRATIONRECORD chain_head( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    return (PEXCEPTIONREGISTRATIONRECORD)ptib->tib_pexchain;
}

#define UNWIND_HANDLER( name, letter )                                           \
static ULONG APIENTRY name( PEXCEPTIONREPORTRECORD rep,                          \
                            PEXCEPTIONREGISTRATIONRECORD reg,                    \
                            PCONTEXTRECORD ctx, PVOID unused )                   \
{                                                                                \
    (void)reg; (void)ctx; (void)unused;                                          \
    if( rep->fHandlerFlags & ( EH_UNWINDING | EH_EXIT_UNWIND ) ) {               \
        note( letter, rep );                                                     \
    }                                                                            \
    return XCPT_CONTINUE_SEARCH;                                                 \
}

UNWIND_HANDLER( handler_o, 'O' )
UNWIND_HANDLER( handler_m, 'M' )
UNWIND_HANDLER( handler_i, 'I' )
UNWIND_HANDLER( handler_a, 'A' )
UNWIND_HANDLER( handler_b, 'B' )

/* ---------- 1, 2: DosUnwindException to a record ---------- */

static EXCEPTIONREGISTRATIONRECORD *target;
static EXCEPTIONREPORTRECORD given;
static ULONG unwind_rc;
static PEXCEPTIONREGISTRATIONRECORD head_after;
static int use_given;

static void inner( void )
{
    EXCEPTIONREGISTRATIONRECORD i;
    i.prev_structure = NULL;
    i.ExceptionHandler = (ERR)handler_i;
    DosSetExceptionHandler( &i );
    unwind_rc = DosUnwindException( target, NULL, use_given ? &given : NULL );
    head_after = chain_head();
    /* I and M are off the chain now: nothing to unset. */
}

static void middle( void )
{
    EXCEPTIONREGISTRATIONRECORD m;
    m.prev_structure = NULL;
    m.ExceptionHandler = (ERR)handler_m;
    DosSetExceptionHandler( &m );
    inner();
}

static void unwind_to_a_record( int with_report )
{
    EXCEPTIONREGISTRATIONRECORD o;
    o.prev_structure = NULL;
    o.ExceptionHandler = (ERR)handler_o;
    DosSetExceptionHandler( &o );
    target = &o;
    use_given = with_report;
    memset( &given, 0, sizeof( given ) );
    given.ExceptionNum = TEST_XCPT;
    reset_trace();
    middle();
    if( !with_report ) {
        check( "DosUnwindException( &O, NULL, NULL ) returns NO_ERROR", unwind_rc, NO_ERROR );
        check_str( "  called I, then M", trace, "IM" );
        check( "  each with EH_UNWINDING", all_unwinding, 1 );
        check( "  with a report record of XCPT_UNWIND", last_num == XCPT_UNWIND, 1 );
        check( "  O is the head again", head_after == &o, 1 );
    } else {
        check( "DosUnwindException with a report record", unwind_rc, NO_ERROR );
        check( "  the handlers get that record", last_rep == &given, 1 );
        check( "  its code unchanged", given.ExceptionNum == TEST_XCPT, 1 );
        check( "  EH_UNWINDING added to its flags", ( given.fHandlerFlags & EH_UNWINDING ) != 0, 1 );
    }
    check( "  O was not called", strchr( trace, 'O' ) == NULL, 1 );
    DosUnsetExceptionHandler( &o );
}

/* ---------- 3: longjmp unwinds the handlers registered since setjmp ---------- */

static jmp_buf env;

static void jump_inner( int how )
{
    EXCEPTIONREGISTRATIONRECORD i;
    i.prev_structure = NULL;
    i.ExceptionHandler = (ERR)handler_i;
    DosSetExceptionHandler( &i );
    longjmp( env, how );
}

static void jump_middle( int how )
{
    EXCEPTIONREGISTRATIONRECORD m;
    m.prev_structure = NULL;
    m.ExceptionHandler = (ERR)handler_m;
    DosSetExceptionHandler( &m );
    jump_inner( how );
}

static void longjmp_unwinds( void )
{
    PEXCEPTIONREGISTRATIONRECORD before = chain_head();
    volatile int r;
    reset_trace();
    r = setjmp( env );
    if( r == 0 ) {
        jump_middle( 7 );
    }
    check( "longjmp returns to setjmp", r, 7 );
    check_str( "  having unwound I, then M", trace, "IM" );
    check( "  the chain is as it was at setjmp", chain_head() == before, 1 );
}

/* ---------- 4: leaving an access violation by longjmp ---------- */

static jmp_buf fault_env;
static volatile ULONG faults;

static ULONG APIENTRY fault_handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                                     PCONTEXTRECORD ctx, PVOID unused )
{
    (void)reg; (void)ctx; (void)unused;
    if( rep->fHandlerFlags & ( EH_UNWINDING | EH_EXIT_UNWIND ) ) {
        note( 'F', rep );
        return XCPT_CONTINUE_SEARCH;
    }
    if( rep->ExceptionNum == XCPT_ACCESS_VIOLATION ) {
        faults++;
        longjmp( fault_env, 1 );
    }
    return XCPT_CONTINUE_SEARCH;
}

static int *volatile nowhere = NULL;

static void faulting_frame( void )
{
    EXCEPTIONREGISTRATIONRECORD f;
    f.prev_structure = NULL;
    f.ExceptionHandler = (ERR)fault_handler;
    DosSetExceptionHandler( &f );
    *nowhere = 1;                      /* access violation */
    DosUnsetExceptionHandler( &f );
}

static ULONG APIENTRY continue_handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                                        PCONTEXTRECORD ctx, PVOID unused )
{
    (void)reg; (void)ctx; (void)unused;
    if( rep->ExceptionNum == TEST_XCPT && !( rep->fHandlerFlags & ( EH_UNWINDING | EH_EXIT_UNWIND ) ) ) {
        last_rep = rep;
        last_num = (ULONG)rep->ExceptionAddress;
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

static ULONG raise_rc;

/* Raises TEST_XCPT; its handler continues. Not inlined, so that the
 * raise is in the function whose address the check compares with. */
#ifdef __GNUC__
__attribute__((noinline))
#endif
static void raising_function( void )
{
    EXCEPTIONREPORTRECORD rep;
    memset( &rep, 0, sizeof( rep ) );
    rep.ExceptionNum = TEST_XCPT;
    raise_rc = DosRaiseException( &rep );
}

static void leave_by_longjmp( void )
{
    PEXCEPTIONREGISTRATIONRECORD before = chain_head();
    EXCEPTIONREGISTRATIONRECORD c;
    int round;
    ULONG raised_at, fn;

    for( round = 0; round < 3; round++ ) {
        reset_trace();
        if( setjmp( fault_env ) == 0 ) {
            faulting_frame();
            check( "the access violation reached the handler", 0, 1 );
        }
    }
    check( "an access violation left by longjmp three times", faults, 3 );
    check_str( "  each time unwinding the handler itself", trace, "F" );
    check( "  the chain is as it was", chain_head() == before, 1 );

    /* Exceptions still work after handlers were left. */
    c.prev_structure = NULL;
    c.ExceptionHandler = (ERR)continue_handler;
    DosSetExceptionHandler( &c );
    reset_trace();
    raising_function();
    DosUnsetExceptionHandler( &c );
    check( "afterwards, DosRaiseException continued returns NO_ERROR", raise_rc, NO_ERROR );
    raised_at = last_num;
    fn = (ULONG)raising_function;
    check( "  its ExceptionAddress lies in the raising function",
           raised_at > fn && raised_at < fn + 0x400, 1 );
}

/* ---------- 5: a thread unwinds its whole chain ---------- */

static HEV thread_done;
static ULONG t_rc_end, t_rc_all;
static PEXCEPTIONREGISTRATIONRECORD t_head_end, t_head_all;
static char t_trace_end[16], t_trace_all[16];

static void APIENTRY whole_chain_thread( ULONG arg )
{
    EXCEPTIONREGISTRATIONRECORD a, b;
    (void)arg;
    a.prev_structure = NULL;
    a.ExceptionHandler = (ERR)handler_a;
    b.prev_structure = NULL;
    b.ExceptionHandler = (ERR)handler_b;

    reset_trace();
    DosSetExceptionHandler( &a );
    DosSetExceptionHandler( &b );
    t_rc_end = DosUnwindException( CHAIN_END, NULL, NULL );
    t_head_end = chain_head();
    strcpy( t_trace_end, trace );

    reset_trace();
    DosSetExceptionHandler( &a );
    DosSetExceptionHandler( &b );
    t_rc_all = DosUnwindException( (PEXCEPTIONREGISTRATIONRECORD)0, NULL, NULL );
    t_head_all = chain_head();
    strcpy( t_trace_all, trace );

    DosPostEventSem( thread_done );
}

static void whole_chain( void )
{
    TID tid;
    DosCreateEventSem( NULL, &thread_done, 0, FALSE );
    DosCreateThread( &tid, whole_chain_thread, 0, CREATE_READY | STACK_COMMITTED, STACK_SIZE );
    DosWaitEventSem( thread_done, 10000 );
    check( "END_OF_CHAIN returns NO_ERROR", t_rc_end, NO_ERROR );
    check_str( "  called B, then A", t_trace_end, "BA" );
    check( "  the chain is empty", t_head_end == CHAIN_END, 1 );
    check( "UNWIND_ALL (0) returns NO_ERROR", t_rc_all, NO_ERROR );
    check_str( "  called B, then A", t_trace_all, "BA" );
    check( "  the chain is empty", t_head_all == CHAIN_END, 1 );
    DosCloseEventSem( thread_done );
}

/* ---------- 6: a target that is not on the chain ---------- */

static void not_on_the_chain( void )
{
    EXCEPTIONREGISTRATIONRECORD m, stray;
    PEXCEPTIONREGISTRATIONRECORD before;
    ULONG rc;
    m.prev_structure = NULL;
    m.ExceptionHandler = (ERR)handler_m;
    stray.prev_structure = NULL;
    stray.ExceptionHandler = (ERR)handler_o;
    DosSetExceptionHandler( &m );
    before = chain_head();
    reset_trace();
    rc = DosUnwindException( &stray, NULL, NULL );
    check( "a target not on the chain is refused", rc != NO_ERROR, 1 );
    check_str( "  no handler was called", trace, "" );
    check( "  the chain is unchanged", chain_head() == before, 1 );
    DosUnsetExceptionHandler( &m );
}

int main( void )
{
    char line[64];
    char *p;

    unwind_to_a_record( 0 );
    unwind_to_a_record( 1 );
    longjmp_unwinds();
    leave_by_longjmp();
    whole_chain();
    not_on_the_chain();

    p = append_str( line, "xcpt_unwind_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );
    return n_passed == n_checks ? 0 : 1;
}
