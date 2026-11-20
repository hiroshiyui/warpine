/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_scroll_test.c - scroll-bar controls and the PM text cursor under Warpine
 *
 * The client holds a vertical scroll bar (id 1, range 0..100 set with
 * SBM_SETSCROLLBAR, thumb size 20 of 120) and a horizontal one (id 2, range
 * 0..10 from SBCDATA control data). Like a real application it scrolls by
 * itself: on WM_VSCROLL / WM_HSCROLL it moves the slider with SBM_SETPOS (a
 * line is 1, a page 10 on the vertical bar and 3 on the horizontal one; a
 * dragged slider is followed on SB_SLIDERPOSITION). Every notification is
 * logged and the log is checked at the end. The integration test clicks:
 *
 *   the up arrow, the down arrow, the shaft above the slider (vertical);
 *   drags the slider 70 pels down (position 40 -> 90);
 *   the right arrow and the shaft right of the slider (horizontal);
 *
 * then closes the window with Alt+F4. Before that, main() exercises
 * WinCreateCursor / WinShowCursor / WinQueryCursorInfo / CURSOR_SETPOS and
 * leaves a frame cursor showing over a blue box; WinDestroyCursor runs at
 * the end. Each check is written to stdout:
 *
 *   pm_scroll_test: <name>: ok
 *   pm_scroll_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_scroll_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_DOSPROCESS
#include <os2.h>
#include <string.h>

#define ID_VBAR   1
#define ID_HBAR   2
#define MAX_LOG   64

static HAB  hab;
static HWND frame, client, vbar, hbar;
static int  n_checks = 0;
static int  n_passed = 0;

/* The notifications received: command, slider, scroll bar id, message. */
static USHORT log_cmd[MAX_LOG];
static SHORT  log_slider[MAX_LOG];
static USHORT log_id[MAX_LOG];
static ULONG  log_msg[MAX_LOG];
static int    n_log = 0;

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

static void check( const char *what, LONG got, LONG want )
{
    char  line[200], *p;
    p = append_str( line, "pm_scroll_test: " );
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

static void check_n( const char *what, int n, LONG got, LONG want )
{
    char name[80], *p;
    p = append_str( name, what );
    p = append_str( p, " #" );
    append_long( p, n );
    check( name, got, want );
}

/* ---------- the client ---------- */

static void scroll( HWND bar, MPARAM mp2, SHORT line, SHORT page )
{
    SHORT pos = (SHORT)SHORT1FROMMR( WinSendMsg( bar, SBM_QUERYPOS, 0, 0 ) );
    switch( SHORT2FROMMP( mp2 ) ) {
    case SB_LINEUP:         pos -= line; break;
    case SB_LINEDOWN:       pos += line; break;
    case SB_PAGEUP:         pos -= page; break;
    case SB_PAGEDOWN:       pos += page; break;
    case SB_SLIDERPOSITION: pos = SHORT1FROMMP( mp2 ); break;
    default:                return;
    }
    WinSendMsg( bar, SBM_SETPOS, MPFROMSHORT( pos ), 0 );
}

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_VSCROLL:
    case WM_HSCROLL:
        if( n_log < MAX_LOG ) {
            log_msg[n_log] = msg;
            log_id[n_log] = SHORT1FROMMP( mp1 );
            log_slider[n_log] = SHORT1FROMMP( mp2 );
            log_cmd[n_log] = SHORT2FROMMP( mp2 );
            n_log++;
        }
        if( msg == WM_VSCROLL )
            scroll( vbar, mp2, 1, 10 );
        else
            scroll( hbar, mp2, 1, 3 );
        return 0;

    case WM_PAINT: {
        RECTL rc, box = { 20, 370, 60, 400 };
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinFillRect( hps, &rc, CLR_WHITE );
        WinFillRect( hps, &box, CLR_BLUE );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

/* Expected notification `i`: message, id, command; the slider is 0 for
 * the line and page commands and the end of scrolling. */
static int expect( int i, ULONG msg, USHORT id, USHORT cmd, const char *what )
{
    if( i >= n_log ) {
        check_n( what, i, -1, cmd );
        return i;
    }
    check_n( what, i, log_cmd[i], cmd );
    check_n( "  message", i, log_msg[i], msg );
    check_n( "  id", i, log_id[i], id );
    if( cmd != SB_SLIDERTRACK && cmd != SB_SLIDERPOSITION )
        check_n( "  slider 0", i, log_slider[i], 0 );
    return i + 1;
}

static void check_log( void )
{
    int i = 0, tracks = 0;

    i = expect( i, WM_VSCROLL, ID_VBAR, SB_LINEUP, "up arrow: SB_LINEUP" );
    i = expect( i, WM_VSCROLL, ID_VBAR, SB_ENDSCROLL, "up arrow released: SB_ENDSCROLL" );
    i = expect( i, WM_VSCROLL, ID_VBAR, SB_LINEDOWN, "down arrow: SB_LINEDOWN" );
    i = expect( i, WM_VSCROLL, ID_VBAR, SB_ENDSCROLL, "down arrow released: SB_ENDSCROLL" );
    i = expect( i, WM_VSCROLL, ID_VBAR, SB_PAGEUP, "shaft above the slider: SB_PAGEUP" );
    i = expect( i, WM_VSCROLL, ID_VBAR, SB_ENDSCROLL, "shaft released: SB_ENDSCROLL" );
    while( i < n_log && log_cmd[i] == SB_SLIDERTRACK ) {
        tracks++;
        i++;
    }
    check( "slider drag: SB_SLIDERTRACK sent", tracks > 0, TRUE );
    check( "slider drag: last tracked position", i > 0 ? log_slider[i - 1] : -1, 90 );
    i = expect( i, WM_VSCROLL, ID_VBAR, SB_SLIDERPOSITION, "slider released: SB_SLIDERPOSITION" );
    check( "slider released: final position", i > 0 ? log_slider[i - 1] : -1, 90 );
    i = expect( i, WM_HSCROLL, ID_HBAR, SB_LINERIGHT, "right arrow: SB_LINERIGHT" );
    i = expect( i, WM_HSCROLL, ID_HBAR, SB_ENDSCROLL, "right arrow released: SB_ENDSCROLL" );
    i = expect( i, WM_HSCROLL, ID_HBAR, SB_PAGERIGHT, "shaft right of the slider: SB_PAGERIGHT" );
    i = expect( i, WM_HSCROLL, ID_HBAR, SB_ENDSCROLL, "shaft released: SB_ENDSCROLL" );
    check( "no other notifications", n_log, i );

    check( "vertical position at the end",
           SHORT1FROMMR( WinSendMsg( vbar, SBM_QUERYPOS, 0, 0 ) ), 90 );
    check( "horizontal position at the end",
           SHORT1FROMMR( WinSendMsg( hbar, SBM_QUERYPOS, 0, 0 ) ), 4 );
}

/* ---------- scroll-bar messages without the pointer ---------- */

static void check_messages( void )
{
    MRESULT range;

    WinSendMsg( vbar, SBM_SETSCROLLBAR, MPFROMSHORT( 500 ), MPFROM2SHORT( 5, 60 ) );
    check( "SBM_SETSCROLLBAR clamps the position",
           SHORT1FROMMR( WinSendMsg( vbar, SBM_QUERYPOS, 0, 0 ) ), 60 );
    range = WinSendMsg( vbar, SBM_QUERYRANGE, 0, 0 );
    check( "SBM_QUERYRANGE first", SHORT1FROMMR( range ), 5 );
    check( "SBM_QUERYRANGE last", SHORT2FROMMR( range ), 60 );
    WinSendMsg( vbar, SBM_SETPOS, MPFROMSHORT( 1 ), 0 );
    check( "SBM_SETPOS clamps to the range",
           SHORT1FROMMR( WinSendMsg( vbar, SBM_QUERYPOS, 0, 0 ) ), 5 );
    check( "SBM_SETTHUMBSIZE",
           (LONG)WinSendMsg( vbar, SBM_SETTHUMBSIZE, MPFROM2SHORT( 20, 120 ), 0 ), TRUE );

    /* The range the test session scrolls in. */
    WinSendMsg( vbar, SBM_SETSCROLLBAR, MPFROMSHORT( 50 ), MPFROM2SHORT( 0, 100 ) );
    range = WinSendMsg( hbar, SBM_QUERYRANGE, 0, 0 );
    check( "SBCDATA range first", SHORT1FROMMR( range ), 0 );
    check( "SBCDATA range last", SHORT2FROMMR( range ), 10 );
    check( "SBCDATA position", SHORT1FROMMR( WinSendMsg( hbar, SBM_QUERYPOS, 0, 0 ) ), 0 );
}

/* ---------- the text cursor ---------- */

static void check_cursor( void )
{
    RECTL      clip = { 0, 0, 200, 450 };
    CURSORINFO ci;

    check( "WinShowCursor without a cursor", WinShowCursor( client, TRUE ), FALSE );
    check( "WinQueryCursorInfo without a cursor", WinQueryCursorInfo( HWND_DESKTOP, &ci ), FALSE );
    check( "WinCreateCursor", WinCreateCursor( client, 20, 400, 8, 16, CURSOR_FRAME, &clip ), TRUE );
    check( "WinQueryCursorInfo", WinQueryCursorInfo( HWND_DESKTOP, &ci ), TRUE );
    check( "  hwnd", ci.hwnd == client, TRUE );
    check( "  x", ci.x, 20 );
    check( "  y", ci.y, 400 );
    check( "  cx", ci.cx, 8 );
    check( "  cy", ci.cy, 16 );
    check( "  fs", ci.fs, CURSOR_FRAME );
    check( "  clip xRight", ci.rclClip.xRight, 200 );
    check( "  clip yTop", ci.rclClip.yTop, 450 );
    check( "WinShowCursor shows the new (hidden) cursor", WinShowCursor( client, TRUE ), TRUE );
    check( "WinShowCursor of a visible cursor", WinShowCursor( client, TRUE ), FALSE );
    check( "WinShowCursor(FALSE)", WinShowCursor( client, FALSE ), TRUE );
    check( "WinShowCursor(TRUE) again", WinShowCursor( client, TRUE ), TRUE );
    check( "WinShowCursor of another window", WinShowCursor( frame, TRUE ), FALSE );
    check( "CURSOR_SETPOS", WinCreateCursor( client, 30, 380, 0, 0, CURSOR_SETPOS, NULL ), TRUE );
    WinQueryCursorInfo( HWND_DESKTOP, &ci );
    check( "CURSOR_SETPOS moved x", ci.x, 30 );
    check( "CURSOR_SETPOS moved y", ci.y, 380 );
    check( "CURSOR_SETPOS kept cx", ci.cx, 8 );
    check( "CURSOR_SETPOS kept the flags", ci.fs, CURSOR_FRAME );
}

int main( void )
{
    HMQ     hmq;
    QMSG    qmsg;
    ULONG   flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_BORDER;
    SBCDATA sbc;
    CURSORINFO ci;
    char    line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "ScrollTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "ScrollTest",
                                "PM Scroll Bar Test", WS_VISIBLE, NULLHANDLE, 0, &client );

    vbar = WinCreateWindow( client, WC_SCROLLBAR, NULL, WS_VISIBLE | SBS_VERT,
                            400, 100, 16, 200, client, HWND_TOP, ID_VBAR, NULL, NULL );
    memset( &sbc, 0, sizeof( sbc ) );
    sbc.cb = sizeof( sbc );
    sbc.posFirst = 0;
    sbc.posLast = 10;
    sbc.posThumb = 0;
    hbar = WinCreateWindow( client, WC_SCROLLBAR, NULL, WS_VISIBLE | SBS_HORZ,
                            50, 40, 200, 16, client, HWND_TOP, ID_HBAR, &sbc, NULL );
    check( "WinCreateWindow(WC_SCROLLBAR)", vbar != NULLHANDLE && hbar != NULLHANDLE, TRUE );

    check_messages();
    check_cursor();

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    check_log();
    check( "WinDestroyCursor of another window", WinDestroyCursor( frame ), FALSE );
    check( "WinDestroyCursor", WinDestroyCursor( client ), TRUE );
    check( "no cursor after WinDestroyCursor", WinQueryCursorInfo( HWND_DESKTOP, &ci ), FALSE );

    p = append_str( line, "pm_scroll_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
