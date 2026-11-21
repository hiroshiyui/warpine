/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * wcxx_throw_test.cpp - Open Watcom C++ exceptions on Warpine (ADR-0110)
 *
 * Open Watcom's C++ runtime registers an OS/2 exception handler for each
 * C++ frame. A throw raises an exception (DosRaiseException) to find the
 * catch, then unwinds with DosUnwindException to the catching frame: the
 * handlers of the frames in between are called with EH_UNWINDING and run
 * their destructors. So these checks hold only when Warpine's
 * DosUnwindException calls the handlers and goes on where it is told:
 *   - a throw through three frames with locals runs their destructors,
 *     innermost first, and the catch gets the value;
 *   - a rethrow (throw;) runs the destructors of the frames it leaves;
 *   - a throw caught by an inner try leaves the outer frames alone;
 *   - an OS/2 handler a C-style frame registered between thrower and
 *     catcher is called with EH_UNWINDING and leaves the chain;
 *   - a hardware exception does not end the program while a C++ try is
 *     active (Open Watcom's handlers pass it on) and a later throw works.
 *
 * Each check is written to stdout:
 *
 *   wcxx_throw_test: <name>: ok
 *   wcxx_throw_test: <name>: FAIL got <value> want <value>
 *
 * followed by "wcxx_throw_test: done <passed>/<total>".
 */
#define INCL_DOSPROCESS
#define INCL_DOSEXCEPTIONS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <string.h>

static int n_checks = 0;
static int n_passed = 0;

static void check( const char *what, long got, long want )
{
    n_checks++;
    if( got == want ) {
        n_passed++;
        printf( "wcxx_throw_test: %s: ok\n", what );
    } else {
        printf( "wcxx_throw_test: %s: FAIL got %ld want %ld\n", what, got, want );
    }
}

static void check_str( const char *what, const char *got, const char *want )
{
    n_checks++;
    if( strcmp( got, want ) == 0 ) {
        n_passed++;
        printf( "wcxx_throw_test: %s: ok\n", what );
    } else {
        printf( "wcxx_throw_test: %s: FAIL got \"%s\" want \"%s\"\n", what, got, want );
    }
}

/* What the destructors ran, in order. */
static char log_[64];

static void note( char c )
{
    size_t n = strlen( log_ );
    if( n < sizeof( log_ ) - 1 ) {
        log_[n] = c;
        log_[n + 1] = '\0';
    }
}

struct Guard {
    char name;
    Guard( char n ) : name( n ) {}
    ~Guard() { note( name ); }
};

struct Problem {
    int code;
    Problem( int c ) : code( c ) {}
};

/* ---------- 1: through three frames ---------- */

static void level3( int code )
{
    Guard g( '3' );
    throw Problem( code );
}

static void level2( int code )
{
    Guard g( '2' );
    level3( code );
    note( 'X' );                       /* never reached */
}

static void level1( int code )
{
    Guard g( '1' );
    level2( code );
}

static void through_three_frames( void )
{
    int caught = 0;
    log_[0] = '\0';
    try {
        Guard outer( 'O' );
        level1( 42 );
    } catch( Problem &p ) {
        caught = p.code;
    }
    check( "a throw through three frames is caught", caught, 42 );
    check_str( "  their destructors ran, innermost first", log_, "321O" );
}

/* ---------- 2: rethrow ---------- */

static void rethrower( void )
{
    Guard g( 'R' );
    try {
        Guard inner( 'i' );
        throw Problem( 7 );
    } catch( Problem & ) {
        note( 'c' );
        throw;
    }
}

static void rethrow( void )
{
    int caught = 0;
    log_[0] = '\0';
    try {
        rethrower();
    } catch( Problem &p ) {
        caught = p.code;
    }
    check( "a rethrow reaches the outer catch", caught, 7 );
    check_str( "  destructors: the try's, the catch, the frame's", log_, "icR" );
}

/* ---------- 3: caught inside ---------- */

static int caught_inside( void )
{
    Guard g( 'F' );
    int code = 0;
    try {
        level3( 9 );
    } catch( Problem &p ) {
        code = p.code;
    }
    note( 'A' );                       /* after the catch, still in the frame */
    return code;
}

static void inner_catch( void )
{
    log_[0] = '\0';
    check( "a throw caught by an inner try", caught_inside(), 9 );
    check_str( "  the outer frame goes on, then ends", log_, "3AF" );
}

/* ---------- 4: an OS/2 handler between thrower and catcher ---------- */

static int unwound_calls;
static int unwound_flags_ok;

static ULONG APIENTRY os2_handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD,
                                   PCONTEXTRECORD, PVOID )
{
    if( rep->fHandlerFlags & EH_UNWINDING ) {
        unwound_calls++;
        unwound_flags_ok = 1;
    }
    return XCPT_CONTINUE_SEARCH;
}

static PEXCEPTIONREGISTRATIONRECORD chain_head( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    return (PEXCEPTIONREGISTRATIONRECORD)ptib->tib_pexchain;
}

static void c_style_frame( void )
{
    EXCEPTIONREGISTRATIONRECORD rec;
    rec.prev_structure = NULL;
    rec.ExceptionHandler = (ERR)os2_handler;
    DosSetExceptionHandler( &rec );
    level3( 5 );
    DosUnsetExceptionHandler( &rec );  /* never reached */
}

static void os2_handler_between( void )
{
    PEXCEPTIONREGISTRATIONRECORD before = chain_head();
    int caught = 0;
    log_[0] = '\0';
    try {
        c_style_frame();
    } catch( Problem &p ) {
        caught = p.code;
    }
    check( "a throw past a frame with an OS/2 handler", caught, 5 );
    check( "  that handler was called while unwinding", unwound_calls, 1 );
    check( "  with EH_UNWINDING", unwound_flags_ok, 1 );
    check( "  and left the chain", chain_head() == before, 1 );
}

/* ---------- 5: a hardware exception under a try ---------- */

static int *volatile nowhere = NULL;
static int fault_seen;

static ULONG APIENTRY fault_handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD,
                                     PCONTEXTRECORD ctx, PVOID )
{
    if( rep->ExceptionNum == XCPT_ACCESS_VIOLATION && !( rep->fHandlerFlags & EH_UNWINDING ) ) {
        fault_seen = 1;
        ctx->ctx_RegEip += 6;          /* past "mov dword ptr [eax], 1" */
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

static void hardware_then_throw( void )
{
    EXCEPTIONREGISTRATIONRECORD rec;
    int caught = 0;
    rec.prev_structure = NULL;
    rec.ExceptionHandler = (ERR)fault_handler;
    try {
        Guard g( 'H' );
        DosSetExceptionHandler( &rec );
        _asm {
            mov eax, nowhere
            mov dword ptr [eax], 1
        }
        DosUnsetExceptionHandler( &rec );
        throw Problem( 11 );
    } catch( Problem &p ) {
        caught = p.code;
    }
    check( "an access violation under a try reaches the OS/2 handler", fault_seen, 1 );
    check( "  and a throw after it is caught", caught, 11 );
}

int main( void )
{
    through_three_frames();
    rethrow();
    inner_catch();
    os2_handler_between();
    hardware_then_throw();
    printf( "wcxx_throw_test: done %d/%d\n", n_passed, n_checks );
    return n_passed == n_checks ? 0 : 1;
}
