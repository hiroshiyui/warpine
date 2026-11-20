/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_twoproc_test.c - two PM programs on one desktop, under Warpine
 *
 * Warpine runs every OS/2 process as a host process of its own; the
 * session's desktop server (ADR-0052) puts their windows on one desktop and
 * hands out input in order across them, as PM's single system input queue
 * does ("System Message Queue": input events are "strictly ordered so that a
 * new event cannot be processed until all previous events are fully
 * processed").
 *
 * The program starts itself again as a child (the argument "child" in
 * pib_pchcmd, never in argv). The parent shows frame A on the left, the
 * child frame B on the right. They share a log in named shared memory: each
 * input event either program reads appends "<A|B><char>" ('1' for a
 * button-1 click). The integration test clicks A, then types:
 *
 *   'w' -> A, which then stays busy for 2 s
 *   'x' -> A (waits while A is busy)
 *   a click on B, 'y' -> B, but only after A has read 'x'
 *   a click on A, 'z' -> A; 'q' -> A ends the test
 *
 * so the log reads A1 Aw Ax B1 By A1 Az Aq. The parent checks it, that the
 * two programs' window handles differ in their desktop slot, that B was the
 * active window when it got 'y' and lost activation when A was clicked, and
 * that A is active at the end. Each check is written to stdout:
 *
 *   pm_twoproc_test: <name>: ok
 *   pm_twoproc_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_twoproc_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include <stdio.h>

#define SHARED_NAME "\\SHAREMEM\\WARPINE_TWOPROC"
#define ID_POLL     1
#define BUSY_MS     2000
#define POLL_MS     100
#define GIVE_UP     150           /* polls (15 s) */

#define LEFT_X      40
#define RIGHT_X     400
#define FRAME_Y     400
#define FRAME_CX    300
#define FRAME_CY    200

typedef struct {
    char  log[64];
    ULONG child_frame;
    LONG  child_ready;
    LONG  stop;
    LONG  child_done;
    LONG  b_active_at_y;        /* B was the active window when it got 'y' */
    LONG  b_lost_after_y;       /* WM_ACTIVATE FALSE reached B after 'y' */
} SHARED;

static HAB     hab;
static HWND    frame, client;
static SHARED *sh;
static int     is_child;
static int     got_y = 0;
static int     polls = 0;
static int     n_checks = 0, n_passed = 0;

static void write_stdout( const char *s )
{
    ULONG written;
    DosWrite( 1, (PVOID)s, strlen( s ), &written );
}

static void report( const char *what, BOOL ok, const char *got, const char *want )
{
    char line[200];
    if( ok ) {
        sprintf( line, "pm_twoproc_test: %s: ok\r\n", what );
        n_passed++;
    } else {
        sprintf( line, "pm_twoproc_test: %s: FAIL got %s want %s\r\n", what, got, want );
    }
    write_stdout( line );
    n_checks++;
}

static void check( const char *what, LONG got, LONG want )
{
    char g[16], w[16];
    sprintf( g, "%ld", got );
    sprintf( w, "%ld", want );
    report( what, got == want, g, w );
}

static void record( char c )
{
    size_t n = strlen( sh->log );
    if( n + 2 >= sizeof( sh->log ) ) return;
    sh->log[n] = is_child ? 'B' : 'A';
    sh->log[n + 1] = c;
    sh->log[n + 2] = '\0';
}

/* The command line's arguments: the second string at pib_pchcmd. */
static const char *arguments( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    if( ppib->pib_pchcmd == NULL ) return "";
    return ppib->pib_pchcmd + strlen( ppib->pib_pchcmd ) + 1;
}

static APIRET start_child( void )
{
    PTIB        ptib;
    PPIB        ppib;
    char        me[CCHMAXPATH], args[CCHMAXPATH + 16];
    RESULTCODES res;
    int         n;

    DosGetInfoBlocks( &ptib, &ppib );
    if( DosQueryModuleName( ppib->pib_hmte, sizeof( me ), me ) != NO_ERROR ) return 1;
    memset( args, 0, sizeof( args ) );
    strcpy( args, me );
    n = strlen( me ) + 1;
    strcpy( args + n, "child" );
    return DosExecPgm( NULL, 0, EXEC_ASYNC, (PSZ)args, NULL, &res, (PSZ)me );
}

static void final_checks( void )
{
    char want[] = "A1AwAxB1ByA1AzAq";
    report( "input in order across the two programs", strcmp( sh->log, want ) == 0,
            sh->log[0] ? sh->log : "(nothing)", want );
    check( "the child ran", sh->child_ready, 1 );
    check( "the handles of the two programs differ in their desktop slot",
           sh->child_frame != 0 && ( sh->child_frame >> 20 ) != ( frame >> 20 ), TRUE );
    check( "B was the active window when it got 'y'", sh->b_active_at_y, 1 );
    check( "B lost activation when A was clicked", sh->b_lost_after_y, 1 );
    check( "A is the active window at the end", WinQueryActiveWindow( HWND_DESKTOP ) == frame, TRUE );
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_BUTTON1DOWN:
        record( '1' );
        return (MRESULT)TRUE;

    case WM_ACTIVATE:
        if( is_child && !SHORT1FROMMP( mp1 ) && got_y ) sh->b_lost_after_y = 1;
        break;

    case WM_CHAR: {
        USHORT fs = SHORT1FROMMP( mp1 );
        if( ( fs & KC_CHAR ) && !( fs & KC_KEYUP ) ) {
            char c = (char)SHORT1FROMMP( mp2 );
            record( c );
            if( c == 'w' ) DosSleep( BUSY_MS );
            if( c == 'y' ) {
                got_y = 1;
                sh->b_active_at_y = WinQueryActiveWindow( HWND_DESKTOP ) == frame;
            }
            if( c == 'q' && !is_child ) {
                sh->stop = 1;
                WinStartTimer( hab, hwnd, ID_POLL, POLL_MS );
            }
        }
        return (MRESULT)TRUE;
    }

    case WM_TIMER:
        if( SHORT1FROMMP( mp1 ) != ID_POLL ) break;
        polls++;
        if( is_child ) {
            if( sh->stop ) {
                sh->child_done = 1;
                WinPostMsg( hwnd, WM_QUIT, 0, 0 );
            }
        } else if( sh->child_done || polls > GIVE_UP / 2 ) {
            WinStopTimer( hab, hwnd, ID_POLL );
            final_checks();
            WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        }
        return 0;

    case WM_PAINT: {
        RECTL rc;
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinFillRect( hps, &rc, is_child ? CLR_WHITE : CLR_PALEGRAY );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU;
    char  line[64];
    PVOID mem;

    is_child = strcmp( arguments(), "child" ) == 0;
    if( is_child ) {
        if( DosGetNamedSharedMem( &mem, (PSZ)SHARED_NAME, PAG_READ | PAG_WRITE ) != NO_ERROR ) return 2;
    } else {
        if( DosAllocSharedMem( &mem, (PSZ)SHARED_NAME, sizeof( SHARED ),
                               PAG_COMMIT | PAG_READ | PAG_WRITE ) != NO_ERROR ) return 2;
        memset( mem, 0, sizeof( SHARED ) );
    }
    sh = (SHARED *)mem;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "TwoProcTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "TwoProcTest",
                                is_child ? "Program B" : "Program A", 0, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, is_child ? RIGHT_X : LEFT_X, FRAME_Y, FRAME_CX, FRAME_CY,
                     SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    if( is_child ) {
        sh->child_frame = frame;
        sh->child_ready = 1;
        WinStartTimer( hab, client, ID_POLL, POLL_MS );
    } else if( start_child() != NO_ERROR ) {
        write_stdout( "pm_twoproc_test: cannot start the child\r\n" );
    }

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    if( !is_child ) {
        sprintf( line, "pm_twoproc_test: done %d/%d\r\n", n_passed, n_checks );
        write_stdout( line );
    }
    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
