/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_inputorder_test.c - pointer input routed when it is read, under Warpine
 *
 * The PM Programming Guide ("System Message Queue", "Capturing Mouse
 * Input"): input events are "strictly ordered so that a new event cannot be
 * processed until all previous events are fully processed"; a mouse message
 * goes to "the window that is under the mouse pointer at the time the system
 * reads the mouse input events"; and a key typed after a click on another
 * window goes there "only if that window became active as a result of the
 * mouse click".
 *
 * Two frames, A on the left and B on the right, A active. The program is
 * busy twice (DosSleep) while the integration test types and clicks:
 *
 *   phase 1:  'a', a click on B's client, 'b'
 *             -> 'a' to A, then B is activated (WM_ACTIVATE FALSE to A, TRUE
 *                to B, before the click), the click to B, 'b' to B
 *   phase 2:  a click on B's client, 'c'; before the program reads them it
 *             swaps the two frames, so A now lies under the click
 *             -> the click goes to A and activates it, 'c' to A
 *
 * Each client records what it gets as a pair of characters: its letter and
 * the typed character, '+' / '-' for WM_ACTIVATE TRUE / FALSE, '1' for
 * WM_BUTTON1DOWN. Each check is written to stdout:
 *
 *   pm_inputorder_test: <name>: ok
 *   pm_inputorder_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_inputorder_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#define INCL_DOSPROCESS
#include <os2.h>
#include <string.h>

#define WM_PHASE1   ( WM_USER + 1 )
#define WM_PHASE2   ( WM_USER + 2 )
#define ID_TIMEOUT  1
#define BUSY1_MS    2500
#define BUSY2_MS    3000
#define TIMEOUT_MS  12000

/* Frame rectangles, desktop coordinates (bottom-left origin). */
#define LEFT_X      40
#define RIGHT_X     400
#define FRAME_Y     400
#define FRAME_CX    300
#define FRAME_CY    200

static HAB  hab;
static HWND frameA, frameB, clientA, clientB;
static int  n_checks = 0;
static int  n_passed = 0;
static int  recording = 0;
static int  finished = 0;
static char log1[64];           /* phase 1 */
static char log2[64];           /* phase 2 */
static char *log_now = NULL;

/* ---------- output (no libc printf) ---------- */

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

static void report( const char *what, BOOL ok, const char *got, const char *want )
{
    char  line[200];
    char *p = append_str( line, "pm_inputorder_test: " );
    p = append_str( p, what );
    if( ok ) {
        append_str( p, ": ok\r\n" );
        n_passed++;
    } else {
        p = append_str( p, ": FAIL got " );
        p = append_str( p, got );
        p = append_str( p, " want " );
        p = append_str( p, want );
        append_str( p, "\r\n" );
    }
    write_stdout( line );
    n_checks++;
}

static void check_text( const char *what, const char *got, const char *want )
{
    report( what, strcmp( got, want ) == 0, got[0] ? got : "(nothing)", want );
}

static void check_hwnd( const char *what, HWND got, HWND want, const char *gname, const char *wname )
{
    report( what, got == want, got == want ? wname : gname, wname );
}

/* Record event `c` for client `hwnd` in the current phase's log. */
static void record( HWND hwnd, char c )
{
    size_t n;
    if( !recording || log_now == NULL ) return;
    n = strlen( log_now );
    if( n + 2 >= sizeof( log1 ) ) return;
    log_now[n] = hwnd == clientA ? 'A' : hwnd == clientB ? 'B' : '?';
    log_now[n + 1] = c;
    log_now[n + 2] = '\0';
}

static void finish( void )
{
    HWND active;
    if( finished ) return;
    finished = 1;
    recording = 0;
    check_text( "phase 1: a key, a click activating B, a key", log1, "AaA-B+B1Bb" );
    check_text( "phase 2: the click goes to the frame under the pointer when read", log2, "B-A+A1Ac" );
    active = WinQueryActiveWindow( HWND_DESKTOP );
    check_hwnd( "A is active at the end", active, frameA, active == frameB ? "B" : "other", "A" );
    WinPostMsg( clientA, WM_QUIT, 0, 0 );
}

/* ---------- the clients ---------- */

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PHASE1:
        recording = 1;
        log_now = log1;
        /* Busy: 'a', the click and 'b' wait in the input queue. */
        DosSleep( BUSY1_MS );
        return 0;

    case WM_PHASE2:
        log_now = log2;
        /* Busy: the click and 'c' wait; then the frames swap places. */
        DosSleep( BUSY2_MS );
        WinSetWindowPos( frameA, HWND_TOP, RIGHT_X, FRAME_Y, 0, 0, SWP_MOVE );
        WinSetWindowPos( frameB, HWND_TOP, LEFT_X, FRAME_Y, 0, 0, SWP_MOVE );
        return 0;

    case WM_ACTIVATE:
        record( hwnd, SHORT1FROMMP( mp1 ) ? '+' : '-' );
        break;

    case WM_BUTTON1DOWN:
        record( hwnd, '1' );
        return (MRESULT)TRUE;

    case WM_CHAR: {
        USHORT fs = SHORT1FROMMP( mp1 );
        if( ( fs & KC_CHAR ) && !( fs & KC_KEYUP ) ) {
            char c = (char)SHORT1FROMMP( mp2 );
            record( hwnd, c );
            if( c == 'b' && log_now == log1 )
                WinPostMsg( hwnd, WM_PHASE2, 0, 0 );
            if( c == 'c' )
                finish();
        }
        return (MRESULT)TRUE;
    }

    case WM_TIMER:
        if( SHORT1FROMMP( mp1 ) == ID_TIMEOUT ) {
            WinStopTimer( hab, hwnd, ID_TIMEOUT );
            finish();
            return 0;
        }
        break;

    case WM_PAINT: {
        RECTL rc;
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinFillRect( hps, &rc, hwnd == clientA ? CLR_PALEGRAY : CLR_WHITE );
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
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "InputOrderTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frameB = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "InputOrderTest",
                                 "Frame B", 0, NULLHANDLE, 0, &clientB );
    frameA = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "InputOrderTest",
                                 "Frame A", 0, NULLHANDLE, 0, &clientA );
    WinSetWindowPos( frameB, HWND_TOP, RIGHT_X, FRAME_Y, FRAME_CX, FRAME_CY,
                     SWP_MOVE | SWP_SIZE | SWP_SHOW );
    WinSetWindowPos( frameA, HWND_TOP, LEFT_X, FRAME_Y, FRAME_CX, FRAME_CY,
                     SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    WinStartTimer( hab, clientA, ID_TIMEOUT, TIMEOUT_MS );
    WinPostMsg( clientA, WM_PHASE1, 0, 0 );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    p = append_str( line, "pm_inputorder_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyWindow( frameA );
    WinDestroyWindow( frameB );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
