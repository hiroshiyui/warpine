/*
 * pm_frame_scroll_test.c - frame scroll bars under Warpine
 *
 * A standard frame with FCF_VERTSCROLL | FCF_HORZSCROLL (title bar,
 * sizing border) is placed at desktop (100, 200), 400 x 300. Its
 * FID_VERTSCROLL bar has the range 0..100, its FID_HORZSCROLL bar 0..10.
 * The frame passes the bars' WM_VSCROLL / WM_HSCROLL to the client, which
 * logs them and moves the slider with SBM_SETPOS (a line is 1, a page 10
 * on the vertical bar and 3 on the horizontal one; a dragged slider is
 * followed on SB_SLIDERPOSITION). The client sits above the horizontal
 * bar; its WM_PAINT fills it light grey with a blue box at its (0, 0) and
 * a red one at its top-right corner. The integration test clicks:
 *
 *   the up arrow and the down arrow of the vertical bar, drags its slider
 *   100 pels down, clicks the shaft below the slider;
 *   the right arrow and the shaft right of the slider (horizontal);
 *   the client at its (50, 50);
 *
 * then closes the window with Alt+F4, when the log, the client's place and
 * GpiQueryPel inside and outside the client are checked. Each check is
 * written to stdout:
 *
 *   pm_frame_scroll_test: <name>: ok
 *   pm_frame_scroll_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_frame_scroll_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSPROCESS
#include <os2.h>
#include <string.h>

#define MAX_LOG   64

static HAB  hab;
static HWND frame, client, vbar, hbar;
static int  n_checks = 0;
static int  n_passed = 0;

/* The notifications received: message, receiving window, id, command, slider. */
static ULONG  log_msg[MAX_LOG];
static HWND   log_hwnd[MAX_LOG];
static USHORT log_id[MAX_LOG];
static USHORT log_cmd[MAX_LOG];
static SHORT  log_slider[MAX_LOG];
static int    n_log = 0;

/* The click on the client. */
static LONG click_x = -1, click_y = -1;

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
    p = append_str( line, "pm_frame_scroll_test: " );
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

static void check_log( void );

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_VSCROLL:
    case WM_HSCROLL:
        if( n_log < MAX_LOG ) {
            log_msg[n_log] = msg;
            log_hwnd[n_log] = hwnd;
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
    case WM_BUTTON1DOWN:
        click_x = SHORT1FROMMP( mp1 );
        click_y = SHORT2FROMMP( mp1 );
        return (MRESULT)TRUE;
    case WM_PAINT: {
        RECTL rc, all, box = { 0, 0, 20, 20 }, corner;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinQueryWindowRect( hwnd, &all );
        /* More than the client: the frame keeps it inside. */
        all.xLeft -= 50; all.yBottom -= 50; all.xRight += 50; all.yTop += 50;
        WinFillRect( hps, &all, CLR_PALEGRAY );
        WinFillRect( hps, &box, CLR_BLUE );
        WinQueryWindowRect( hwnd, &corner );
        corner.xLeft = corner.xRight - 20;
        corner.yBottom = corner.yTop - 20;
        WinFillRect( hps, &corner, CLR_RED );
        WinEndPaint( hps );
        return 0;
    }
    case WM_CLOSE:
        check_log();
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
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
    check_n( "  sent to the client", i, log_hwnd[i] == client, TRUE );
    if( cmd != SB_SLIDERTRACK && cmd != SB_SLIDERPOSITION )
        check_n( "  slider 0", i, log_slider[i], 0 );
    return i + 1;
}

static void check_log( void )
{
    int    i = 0, tracks = 0;
    SHORT  dragged;
    SWP    swp;
    HPS    hps;
    POINTL pt;

    i = expect( i, WM_VSCROLL, FID_VERTSCROLL, SB_LINEUP, "up arrow: SB_LINEUP" );
    i = expect( i, WM_VSCROLL, FID_VERTSCROLL, SB_ENDSCROLL, "up arrow released: SB_ENDSCROLL" );
    i = expect( i, WM_VSCROLL, FID_VERTSCROLL, SB_LINEDOWN, "down arrow: SB_LINEDOWN" );
    i = expect( i, WM_VSCROLL, FID_VERTSCROLL, SB_ENDSCROLL, "down arrow released: SB_ENDSCROLL" );
    while( i < n_log && log_cmd[i] == SB_SLIDERTRACK ) {
        tracks++;
        i++;
    }
    check( "slider drag: SB_SLIDERTRACK sent", tracks > 0, TRUE );
    i = expect( i, WM_VSCROLL, FID_VERTSCROLL, SB_SLIDERPOSITION, "slider released: SB_SLIDERPOSITION" );
    dragged = i > 0 ? log_slider[i - 1] : -1;
    check( "slider dragged down", dragged > 20 && dragged < 80, TRUE );
    i = expect( i, WM_VSCROLL, FID_VERTSCROLL, SB_PAGEDOWN, "shaft below the slider: SB_PAGEDOWN" );
    i = expect( i, WM_VSCROLL, FID_VERTSCROLL, SB_ENDSCROLL, "shaft released: SB_ENDSCROLL" );
    i = expect( i, WM_HSCROLL, FID_HORZSCROLL, SB_LINERIGHT, "right arrow: SB_LINERIGHT" );
    i = expect( i, WM_HSCROLL, FID_HORZSCROLL, SB_ENDSCROLL, "right arrow released: SB_ENDSCROLL" );
    i = expect( i, WM_HSCROLL, FID_HORZSCROLL, SB_PAGERIGHT, "shaft right of the slider: SB_PAGERIGHT" );
    i = expect( i, WM_HSCROLL, FID_HORZSCROLL, SB_ENDSCROLL, "shaft released: SB_ENDSCROLL" );
    check( "no other notifications", n_log, i );
    check( "vertical position at the end",
           SHORT1FROMMR( WinSendMsg( vbar, SBM_QUERYPOS, 0, 0 ) ), dragged + 10 );
    check( "horizontal position at the end",
           SHORT1FROMMR( WinSendMsg( hbar, SBM_QUERYPOS, 0, 0 ) ), 4 );

    /* The click on the client, in client coordinates. */
    check( "client click x", click_x, 50 );
    check( "client click y", click_y, 50 );

    /* The client lies above the horizontal bar, left of the vertical one. */
    WinQueryWindowPos( client, &swp );
    check( "client above the horizontal bar", swp.y > 0, TRUE );
    hps = WinGetPS( client );
    pt.x = 5; pt.y = 5;
    check( "GpiQueryPel at the client's (5, 5): blue", GpiQueryPel( hps, &pt ), CLR_BLUE );
    pt.x = swp.cx - 5; pt.y = swp.cy - 5;
    check( "GpiQueryPel at the client's top right: red", GpiQueryPel( hps, &pt ), CLR_RED );
    pt.x = 100; pt.y = 100;
    check( "GpiQueryPel inside: pale grey", GpiQueryPel( hps, &pt ), CLR_PALEGRAY );
    pt.x = 5; pt.y = -5;
    check( "GpiQueryPel below the client (the bar): none", GpiQueryPel( hps, &pt ), CLR_NOINDEX );
    WinReleasePS( hps );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG fl = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_VERTSCROLL | FCF_HORZSCROLL;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "FrameScroll", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &fl, "FrameScroll", "Frame scroll bars",
                                0, NULLHANDLE, 0, &client );
    vbar = WinWindowFromID( frame, FID_VERTSCROLL );
    hbar = WinWindowFromID( frame, FID_HORZSCROLL );
    check( "FID_VERTSCROLL", vbar != NULLHANDLE, TRUE );
    check( "FID_HORZSCROLL", hbar != NULLHANDLE, TRUE );
    WinSendMsg( vbar, SBM_SETSCROLLBAR, MPFROMSHORT( 0 ), MPFROM2SHORT( 0, 100 ) );
    WinSendMsg( hbar, SBM_SETSCROLLBAR, MPFROMSHORT( 0 ), MPFROM2SHORT( 0, 10 ) );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300,
                     SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    p = append_str( line, "pm_frame_scroll_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return n_passed == n_checks ? 0 : 1;
}
