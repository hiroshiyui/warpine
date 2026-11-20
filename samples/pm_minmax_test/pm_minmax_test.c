/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_minmax_test.c - minimizing, maximizing and restoring frames under Warpine
 *
 * Frame A has minimize and maximize buttons, frame B does not. At start-up
 * the client of A checks, through the API:
 *
 *   - WinSetWindowPos with SWP_MAXIMIZE / SWP_MINIMIZE / SWP_RESTORE sets
 *     WS_MAXIMIZED / WS_MINIMIZED, sends WM_MINMAXFRAME to the client first
 *     (window words already set, state not yet changed; a TRUE reply stops
 *     the change), and a maximized frame covers the screen with its sizing
 *     border just outside it
 *   - the restore position in QWS_XRESTORE .. QWS_CYRESTORE, changeable
 *     with WinSetWindowUShort; QWS_XMINIMIZE is -1 until first minimized
 *   - a minimized frame sits low on the screen
 *   - WM_SYSCOMMAND(SC_MINIMIZE) through WinDefWindowProc minimizes a frame
 *     with the buttons and does nothing without them
 *   - the client gets WM_SIZE for every change, and for a plain
 *     WinSetWindowPos(SWP_SIZE) (old and new size); a size that does not
 *     change sends none; a child window gets WM_SIZE for its own size
 *   - WinQueryWindowPos reports SWP_MINIMIZE / SWP_MAXIMIZE in fl for a
 *     minimized / maximized frame and SWP_ACTIVATE for the active one
 *   - minimizing the active frame activates the next frame in Z-order
 *   - frame C has a hide button (FCF_HIDEMAX): WM_SYSCOMMAND(SC_HIDE)
 *     through WinDefWindowProc hides it, and hiding the active frame
 *     activates the next one
 *
 * Then the integration test drives the frame with the mouse and keyboard;
 * each WM_MINMAXFRAME appends X (maximize), N (minimize) or R (restore):
 *
 *   double click on the title bar      X      Alt+F10    X
 *   double click on the title bar      R      Alt+F5     R
 *   Alt+F9                             N      double click on the icon  R
 *   minimize button                    N      double click on the icon  R
 *   maximize button                    X      restore button            R
 *
 * then clicks the hide button of frame C (C is hidden, A becomes active),
 * and finally double-clicks the system-menu icon, which closes the window
 * (WM_SYSCOMMAND SC_CLOSE, then WM_CLOSE). Each check is written to stdout:
 *
 *   pm_minmax_test: <name>: ok
 *   pm_minmax_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_minmax_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#define INCL_DOSPROCESS
#include <os2.h>
#include <string.h>

#define WM_START    ( WM_USER + 1 )
#define WM_CHECK    ( WM_USER + 2 )
#define INTERACTIVE "XRXRNRNRXR"

static HAB  hab;
static HWND frame_a, client_a, frame_b, client_b, frame_c, client_c, child;
static int  n_checks = 0;
static int  n_passed = 0;

static BOOL veto = FALSE;           /* answer WM_MINMAXFRAME with TRUE */
static int  n_minmax = 0;           /* WM_MINMAXFRAME received */
static SWP  last_swp;               /* its SWP */
static ULONG style_at_minmax;       /* frame style when it arrived */
static SHORT xrestore_at_minmax;    /* QWS_XRESTORE when it arrived */
static int  n_size = 0;             /* WM_SIZE received by client A */
#define MAX_SIZES 8
static MPARAM size_mp1[MAX_SIZES], size_mp2[MAX_SIZES]; /* their mp1 / mp2 */
static MPARAM first_size_mp1, first_size_mp2;
static MPARAM resize_mp1, resize_mp2;   /* expected for the plain SWP_SIZE */
static int  n_child_size = 0;       /* WM_SIZE received by the child */
static MPARAM child_size_mp1, child_size_mp2;
static int  n_hide = 0;             /* SC_HIDE received by client C */
static BOOL c_visible_after_hide = TRUE;
static HWND active_after_hide = NULLHANDLE;
static char ops[32];                /* X / N / R per WM_MINMAXFRAME */
static int  n_ops = -1;             /* -1 = not recording */
static BOOL got_sc_close = FALSE;

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
    char *p = append_str( line, "pm_minmax_test: " );
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

static void check( const char *what, LONG got, LONG want )
{
    char g[12], w[12];
    append_long( g, got );
    append_long( w, want );
    report( what, got == want, g, w );
}

/* ---------- helpers ---------- */

static ULONG state_of( HWND frame )
{
    return WinQueryWindowULong( frame, QWL_STYLE ) & ( WS_MINIMIZED | WS_MAXIMIZED );
}

static void set_state( HWND frame, ULONG fl )
{
    WinSetWindowPos( frame, NULLHANDLE, 0, 0, 0, 0, fl );
}

static BOOL visible( HWND hwnd )
{
    return ( WinQueryWindowULong( hwnd, QWL_STYLE ) & WS_VISIBLE ) != 0;
}

static ULONG swp_fl( HWND hwnd )
{
    SWP swp;
    WinQueryWindowPos( hwnd, &swp );
    return swp.fl;
}

static SHORT word( HWND frame, LONG index )
{
    return (SHORT)WinQueryWindowUShort( frame, index );
}

/* ---------- the start-up checks ---------- */

static void api_checks( void )
{
    SWP   orig, swp;
    RECTL rc_before, rc_max;
    LONG  b  = WinQuerySysValue( HWND_DESKTOP, SV_CXSIZEBORDER );
    LONG  cx = WinQuerySysValue( HWND_DESKTOP, SV_CXSCREEN );
    LONG  cy = WinQuerySysValue( HWND_DESKTOP, SV_CYSCREEN );

    WinSetActiveWindow( HWND_DESKTOP, frame_a );
    WinQueryWindowPos( frame_a, &orig );
    WinQueryWindowRect( client_a, &rc_before );
    check( "WinQueryWindowPos fl: SWP_MOVE | SWP_SIZE", orig.fl & ( SWP_MOVE | SWP_SIZE ), SWP_MOVE | SWP_SIZE );
    check( "WinQueryWindowPos fl: neither minimized nor maximized",
           orig.fl & ( SWP_MINIMIZE | SWP_MAXIMIZE ), 0 );
    check( "WinQueryWindowPos fl: SWP_ACTIVATE for the active frame", ( orig.fl & SWP_ACTIVATE ) != 0, TRUE );
    check( "QWS_XMINIMIZE is -1 before the first minimize", word( frame_a, QWS_XMINIMIZE ), -1 );

    /* A TRUE reply to WM_MINMAXFRAME stops the change. */
    veto = TRUE;
    set_state( frame_a, SWP_MAXIMIZE );
    veto = FALSE;
    check( "WM_MINMAXFRAME sent for SWP_MAXIMIZE", n_minmax, 1 );
    check( "its SWP has SWP_MAXIMIZE", ( last_swp.fl & SWP_MAXIMIZE ) != 0, TRUE );
    check( "the state is not changed when it is sent", style_at_minmax & WS_MAXIMIZED, 0 );
    check( "the restore position is set when it is sent", xrestore_at_minmax, orig.x );
    check( "TRUE stops the maximize", state_of( frame_a ), 0 );
    WinQueryWindowPos( frame_a, &swp );
    check( "TRUE leaves the frame where it was", swp.cx, orig.cx );

    /* Maximize: the screen plus the sizing border on every side. */
    set_state( frame_a, SWP_MAXIMIZE );
    check( "SWP_MAXIMIZE sets WS_MAXIMIZED", state_of( frame_a ), WS_MAXIMIZED );
    WinQueryWindowPos( frame_a, &swp );
    check( "maximized x", swp.x, -b );
    check( "maximized y", swp.y, -b );
    check( "maximized cx", swp.cx, cx + 2 * b );
    check( "maximized cy", swp.cy, cy + 2 * b );
    check( "WM_MINMAXFRAME SWP has the new x", last_swp.x, -b );
    check( "WinQueryWindowPos fl: SWP_MAXIMIZE when maximized",
           swp.fl & ( SWP_MINIMIZE | SWP_MAXIMIZE ), SWP_MAXIMIZE );
    check( "QWS_XRESTORE", word( frame_a, QWS_XRESTORE ), orig.x );
    check( "QWS_YRESTORE", word( frame_a, QWS_YRESTORE ), orig.y );
    check( "QWS_CXRESTORE", word( frame_a, QWS_CXRESTORE ), orig.cx );
    check( "QWS_CYRESTORE", word( frame_a, QWS_CYRESTORE ), orig.cy );
    WinQueryWindowRect( client_a, &rc_max );

    set_state( frame_a, SWP_RESTORE );
    check( "SWP_RESTORE clears WS_MAXIMIZED", state_of( frame_a ), 0 );
    WinQueryWindowPos( frame_a, &swp );
    check( "restored to x", swp.x, orig.x );
    check( "restored to cy", swp.cy, orig.cy );
    set_state( frame_a, SWP_RESTORE );
    check( "SWP_RESTORE of a normal frame sends nothing", n_minmax, 3 );

    /* SC_MINIMIZE through WinDefWindowProc (the client's procedure). */
    WinSendMsg( client_a, WM_SYSCOMMAND, MPFROMSHORT( SC_MINIMIZE ), MPFROM2SHORT( CMDSRC_OTHER, FALSE ) );
    check( "SC_MINIMIZE sets WS_MINIMIZED", state_of( frame_a ), WS_MINIMIZED );
    WinQueryWindowPos( frame_a, &swp );
    check( "a minimized frame sits low on the screen", swp.y < cy / 4, TRUE );
    check( "a minimized frame is small", swp.cx < orig.cx && swp.cy < orig.cy, TRUE );
    check( "QWS_XMINIMIZE is its position", word( frame_a, QWS_XMINIMIZE ), swp.x );
    check( "QWS_YMINIMIZE is its position", word( frame_a, QWS_YMINIMIZE ), swp.y );
    check( "WinQueryWindowPos fl: SWP_MINIMIZE when minimized",
           swp.fl & ( SWP_MINIMIZE | SWP_MAXIMIZE ), SWP_MINIMIZE );
    /* Z-order, top first: A, C, B. */
    check( "minimizing the active frame activates the next one (C)",
           WinQueryActiveWindow( HWND_DESKTOP ) == frame_c, TRUE );
    check( "... which then has SWP_ACTIVATE", ( swp_fl( frame_c ) & SWP_ACTIVATE ) != 0, TRUE );
    check( "... and the minimized frame SWP_DEACTIVATE", ( swp_fl( frame_a ) & SWP_DEACTIVATE ) != 0, TRUE );
    set_state( frame_a, SWP_MINIMIZE | SWP_MAXIMIZE );
    check( "SWP_MINIMIZE | SWP_MAXIMIZE does nothing", state_of( frame_a ), WS_MINIMIZED );

    /* A changed restore width is used. */
    WinSetWindowUShort( frame_a, QWS_CXRESTORE, 300 );
    set_state( frame_a, SWP_RESTORE );
    WinQueryWindowPos( frame_a, &swp );
    check( "WinSetWindowUShort(QWS_CXRESTORE) changes the restore width", swp.cx, 300 );
    check( "... and keeps the restore x", swp.x, orig.x );
    /* A plain size change: WM_SIZE with the client's old and new size. */
    resize_mp1 = MPFROM2SHORT( 300, rc_before.yTop );
    resize_mp2 = MPFROM2SHORT( rc_before.xRight, rc_before.yTop );
    WinSetWindowPos( frame_a, NULLHANDLE, orig.x, orig.y, orig.cx, orig.cy, SWP_MOVE | SWP_SIZE );
    /* The same size again: no WM_SIZE. */
    WinSetWindowPos( frame_a, NULLHANDLE, orig.x, orig.y, orig.cx, orig.cy, SWP_MOVE | SWP_SIZE );
    /* A child window is told its own size. */
    WinSetWindowPos( child, NULLHANDLE, 0, 0, 80, 60, SWP_SIZE );

    /* Frame C, active now, hidden by SC_HIDE: A (restored) becomes active. */
    WinSendMsg( client_c, WM_SYSCOMMAND, MPFROMSHORT( SC_HIDE ), MPFROM2SHORT( CMDSRC_OTHER, FALSE ) );
    check( "SC_HIDE hides the frame", visible( frame_c ), FALSE );
    check( "hiding the active frame activates the next one (A)",
           WinQueryActiveWindow( HWND_DESKTOP ) == frame_a, TRUE );
    WinShowWindow( frame_c, TRUE );
    check( "WinShowWindow shows it again", visible( frame_c ), TRUE );
    check( "... without activating it", WinQueryActiveWindow( HWND_DESKTOP ) == frame_a, TRUE );
    n_hide = 0;

    /* Frame B has no minimize / maximize buttons. */
    WinSendMsg( client_b, WM_SYSCOMMAND, MPFROMSHORT( SC_MAXIMIZE ), MPFROM2SHORT( CMDSRC_OTHER, FALSE ) );
    check( "SC_MAXIMIZE without the buttons does nothing", state_of( frame_b ), 0 );
    set_state( frame_b, SWP_MINIMIZE );
    check( "WinSetWindowPos minimizes it all the same", state_of( frame_b ), WS_MINIMIZED );
    set_state( frame_b, SWP_RESTORE );
    check( "and restores it", state_of( frame_b ), 0 );

    /* WM_SIZE: max, restore, min, restore, resize = 5, posted; checked later. */
    first_size_mp1 = MPFROM2SHORT( rc_before.xRight, rc_before.yTop );
    first_size_mp2 = MPFROM2SHORT( rc_max.xRight, rc_max.yTop );
    WinPostMsg( client_a, WM_CHECK, 0, 0 );
}

/* ---------- the clients ---------- */

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_START:
        api_checks();
        return 0;

    case WM_CHECK:
        check( "WM_SIZE for each change", n_size, 5 );
        check( "WM_SIZE old size", (LONG)size_mp1[0], (LONG)first_size_mp1 );
        check( "WM_SIZE new size", (LONG)size_mp2[0], (LONG)first_size_mp2 );
        check( "WM_SIZE for SWP_SIZE: old size", (LONG)size_mp1[4], (LONG)resize_mp1 );
        check( "WM_SIZE for SWP_SIZE: new size", (LONG)size_mp2[4], (LONG)resize_mp2 );
        check( "WM_SIZE to a resized child window", n_child_size, 1 );
        check( "... with its old size", (LONG)child_size_mp1, (LONG)MPFROM2SHORT( 50, 40 ) );
        check( "... and its new size", (LONG)child_size_mp2, (LONG)MPFROM2SHORT( 80, 60 ) );
        n_ops = 0;      /* now the scripted input */
        ops[0] = '\0';
        return 0;

    case WM_SIZE:
        if( n_size < MAX_SIZES ) {
            size_mp1[n_size] = mp1;
            size_mp2[n_size] = mp2;
        }
        n_size++;
        return 0;

    case WM_MINMAXFRAME: {
        PSWP pswp = (PSWP)PVOIDFROMMP( mp1 );
        n_minmax++;
        last_swp = *pswp;
        style_at_minmax = WinQueryWindowULong( frame_a, QWL_STYLE );
        xrestore_at_minmax = word( frame_a, QWS_XRESTORE );
        if( n_ops >= 0 && n_ops < (int)sizeof( ops ) - 1 ) {
            ops[n_ops++] = ( pswp->fl & SWP_MAXIMIZE ) ? 'X' : ( pswp->fl & SWP_MINIMIZE ) ? 'N' : 'R';
            ops[n_ops] = '\0';
        }
        if( veto ) return (MRESULT)TRUE;
        break;
    }

    case WM_SYSCOMMAND:
        if( SHORT1FROMMP( mp1 ) == SC_CLOSE ) got_sc_close = TRUE;
        break;

    case WM_CLOSE: {
        SWP swp;
        report( "title bar, keys, buttons and icon", strcmp( ops, INTERACTIVE ) == 0, ops, INTERACTIVE );
        check( "closed by a double click on the system menu (SC_CLOSE)", got_sc_close, TRUE );
        check( "the frame ends restored", state_of( frame_a ), 0 );
        WinQueryWindowPos( frame_a, &swp );
        check( "at its normal width", swp.cx, 640 );
        check( "... and WinQueryWindowPos says neither minimized nor maximized",
               swp.fl & ( SWP_MINIMIZE | SWP_MAXIMIZE ), 0 );
        check( "the hide button sends SC_HIDE", n_hide, 1 );
        check( "... which hides frame C", c_visible_after_hide, FALSE );
        check( "... and activates frame A", active_after_hide == frame_a, TRUE );
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }

    case WM_PAINT: {
        RECTL rc;
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinFillRect( hps, &rc, CLR_WHITE );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

MRESULT EXPENTRY PlainWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_PAINT ) {
        RECTL rc;
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinFillRect( hps, &rc, CLR_PALEGRAY );
        WinEndPaint( hps );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

/* Frame C (hide button): records what SC_HIDE did. */
MRESULT EXPENTRY HideWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_SYSCOMMAND && SHORT1FROMMP( mp1 ) == SC_HIDE ) {
        MRESULT r = WinDefWindowProc( hwnd, msg, mp1, mp2 );
        n_hide++;
        c_visible_after_hide = visible( frame_c );
        active_after_hide = WinQueryActiveWindow( HWND_DESKTOP );
        return r;
    }
    return PlainWndProc( hwnd, msg, mp1, mp2 );
}

/* A child window of client A, never shown: counts WM_SIZE. */
MRESULT EXPENTRY ChildWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_SIZE ) {
        n_child_size++;
        child_size_mp1 = mp1;
        child_size_mp2 = mp2;
        return 0;
    }
    if( msg == WM_PAINT ) {
        RECTL rc;
        WinEndPaint( WinBeginPaint( hwnd, NULLHANDLE, &rc ) );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags_a = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MINBUTTON | FCF_MAXBUTTON;
    ULONG flags_b = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    ULONG flags_c = FCF_TITLEBAR | FCF_SYSMENU | FCF_BORDER | FCF_HIDEMAX;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "MinMaxTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    WinRegisterClass( hab, "MinMaxPlain", PlainWndProc, CS_SIZEREDRAW, 0 );
    WinRegisterClass( hab, "MinMaxHide", HideWndProc, CS_SIZEREDRAW, 0 );
    WinRegisterClass( hab, "MinMaxChild", ChildWndProc, 0, 0 );
    frame_a = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags_a, "MinMaxTest",
                                  "PM Min/Max Test", 0, NULLHANDLE, 0, &client_a );
    frame_b = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags_b, "MinMaxPlain",
                                  "No buttons", 0, NULLHANDLE, 0, &client_b );
    /* Out of the way of frame A, at the right of the screen. */
    WinSetWindowPos( frame_b, NULLHANDLE, 700, 400, 250, 150, SWP_MOVE | SWP_SIZE );
    frame_c = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags_c, "MinMaxHide",
                                  "Hide button", 0, NULLHANDLE, 0, &client_c );
    /* Below frame B: title bar at desktop y 518-537, hide button x 896-911. */
    WinSetWindowPos( frame_c, NULLHANDLE, 700, 100, 250, 150, SWP_MOVE | SWP_SIZE );
    child = WinCreateWindow( client_a, "MinMaxChild", "", 0, 10, 10, 50, 40,
                             client_a, HWND_TOP, 99, NULL, NULL );
    WinPostMsg( client_a, WM_START, 0, 0 );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    p = append_str( line, "pm_minmax_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyWindow( frame_c );
    WinDestroyWindow( frame_b );
    WinDestroyWindow( frame_a );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
