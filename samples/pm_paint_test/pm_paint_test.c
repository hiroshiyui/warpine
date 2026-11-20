/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_paint_test.c - update rectangles, WinScrollWindow and WinDrawBorder
 *
 * The window checks the painting calls in steps driven by its own
 * messages and writes each check to stdout:
 *
 *   pm_paint_test: <name>: ok
 *   pm_paint_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_paint_test: done <passed>/<total>". The last paint shows
 * the results with a few borders and a scrolled stripe pattern.
 *
 *   1st WM_PAINT  the whole new window is the update rectangle
 *   WM_USER       WinInvalidateRect / WinValidateRect / WinQueryUpdateRect,
 *                 one WM_PAINT for two invalidations, WinScrollWindow,
 *                 WinDrawBorder
 *   2nd WM_PAINT  WinBeginPaint reports the rectangle invalidated last
 *   3rd WM_PAINT  the picture
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSFILEMGR
#define INCL_WINERRORS
#define INCL_GPIERRORS
#include <os2.h>
#include <string.h>

#define N_CHECKS_MAX 64
#define LINE_H       16
#define PREFIX       "pm_paint_test: "
#define ID_KID       77

static HAB   hab;
static int   step = 0;
static int   n_checks = 0;
static int   n_passed = 0;
static char  results[N_CHECKS_MAX][80];
static HWND  hwndKid;

/* ---------- helpers (no libc printf) ---------- */

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

/* Record one check: "<name>: ok" or "<name>: FAIL got <got> want <want>". */
static void check( const char *name, LONG got, LONG want )
{
    char line[128];
    char *p;

    if( n_checks >= N_CHECKS_MAX ) return;
    p = append_str( results[n_checks], name );
    if( got == want ) {
        append_str( p, ": ok" );
        n_passed++;
    } else {
        p = append_str( p, ": FAIL got " );
        p = append_long( p, got );
        p = append_str( p, " want " );
        append_long( p, want );
    }
    strcpy( line, PREFIX );
    strcat( line, results[n_checks] );
    strcat( line, "\r\n" );
    write_stdout( line );
    n_checks++;
}

/* 1 when *r is (l, b, r, t). */
static LONG is_rect( PRECTL r, LONG l, LONG b, LONG rt, LONG t )
{
    return r->xLeft == l && r->yBottom == b && r->xRight == rt && r->yTop == t;
}

static LONG pel_at( HPS hps, LONG x, LONG y )
{
    POINTL pt;
    pt.x = x;
    pt.y = y;
    return GpiQueryPel( hps, &pt );
}

static LONG last_error( void )
{
    return ERRORIDERROR( WinGetLastError( hab ) );
}

/* Whether a WM_PAINT for hwnd (or a child) waits in the queue. */
static LONG paint_waiting( HWND hwnd )
{
    QMSG q;
    return WinPeekMsg( hab, &q, hwnd, WM_PAINT, WM_PAINT, PM_NOREMOVE );
}

/* ---------- the checks ---------- */

static void update_checks( HWND hwnd )
{
    RECTL r, u;
    QMSG  q;

    /* Paint what is still waiting (the child's first WM_PAINT). */
    while( WinPeekMsg( hab, &q, hwnd, WM_PAINT, WM_PAINT, PM_REMOVE ) )
        WinDispatchMsg( hab, &q );
    check( "valid after the first paint", WinQueryUpdateRect( hwnd, &u ), FALSE );
    check( "no WM_PAINT waiting", paint_waiting( hwnd ), FALSE );

    WinSetRect( hab, &r, 10, 10, 50, 40 );
    check( "WinInvalidateRect", WinInvalidateRect( hwnd, &r, FALSE ), TRUE );
    check( "WinQueryUpdateRect", WinQueryUpdateRect( hwnd, &u ), TRUE );
    check( "update rectangle", is_rect( &u, 10, 10, 50, 40 ), 1 );
    WinSetRect( hab, &r, 100, 20, 120, 30 );
    WinInvalidateRect( hwnd, &r, FALSE );
    WinQueryUpdateRect( hwnd, &u );
    check( "second rectangle joins it", is_rect( &u, 10, 10, 120, 40 ), 1 );
    check( "WM_PAINT waiting", paint_waiting( hwnd ), TRUE );

    WinSetRect( hab, &r, 0, 0, 1000, 25 );
    check( "WinValidateRect", WinValidateRect( hwnd, &r, FALSE ), TRUE );
    WinQueryUpdateRect( hwnd, &u );
    check( "validated band removed", is_rect( &u, 10, 25, 120, 40 ), 1 );
    check( "WinQueryUpdateRect NULL rectangle", WinQueryUpdateRect( hwnd, NULL ), TRUE );

    /* One WM_PAINT for both invalidations: painting it validates. */
    check( "WM_PAINT read", WinPeekMsg( hab, &q, hwnd, WM_PAINT, WM_PAINT, PM_REMOVE ), TRUE );
    check( "WM_PAINT for the client", q.hwnd == hwnd, TRUE );
    WinDispatchMsg( hab, &q );
    check( "valid after painting", WinQueryUpdateRect( hwnd, &u ), FALSE );
    check( "only one WM_PAINT", paint_waiting( hwnd ), FALSE );

    WinInvalidateRect( hwnd, NULL, FALSE );
    check( "WinValidateRect NULL", WinValidateRect( hwnd, NULL, FALSE ), TRUE );
    check( "nothing to paint", WinQueryUpdateRect( hwnd, &u ), FALSE );
    check( "WM_PAINT withdrawn", paint_waiting( hwnd ), FALSE );
    check( "WinQueryUpdateRect bad window", WinQueryUpdateRect( (HWND)0xDEAD, &u ), FALSE );
    check( "-> PMERR_INVALID_HWND", last_error(), PMERR_INVALID_HWND );
}

static void scroll_checks( HWND hwnd )
{
    HPS   hps = WinGetPS( hwnd );
    RECTL r, win, upd;
    SWP   swp;

    WinQueryWindowRect( hwnd, &win );
    WinFillRect( hps, &win, CLR_WHITE );
    WinSetRect( hab, &r, 20, 20, 40, 40 );
    WinFillRect( hps, &r, CLR_RED );

    /* Right 30 and up 10, the whole window, nothing invalidated. */
    check( "WinScrollWindow L-shaped",
           WinScrollWindow( hwnd, 30, 10, NULL, NULL, NULLHANDLE, &upd, 0 ), RGN_COMPLEX );
    check( "uncovered bounding rectangle", is_rect( &upd, 0, 0, win.xRight, win.yTop ), 1 );
    check( "pels moved", pel_at( hps, 50, 30 ), CLR_RED );
    check( "moved square's far corner", pel_at( hps, 69, 49 ), CLR_RED );
    check( "beside the moved square", pel_at( hps, 49, 30 ) == CLR_RED, FALSE );
    check( "no SW_INVALIDATERGN: still valid", WinQueryUpdateRect( hwnd, &upd ), FALSE );

    /* Down 10 within the bottom 100 rows, invalidating the uncovered band. */
    WinSetRect( hab, &r, 0, 0, win.xRight, 100 );
    check( "WinScrollWindow band",
           WinScrollWindow( hwnd, 0, -10, &r, NULL, NULLHANDLE, &upd, SW_INVALIDATERGN ), RGN_RECT );
    check( "uncovered band", is_rect( &upd, 0, 90, win.xRight, 100 ), 1 );
    check( "SW_INVALIDATERGN", WinQueryUpdateRect( hwnd, &upd ), TRUE );
    check( "band is the update rectangle", is_rect( &upd, 0, 90, win.xRight, 100 ), 1 );
    check( "pels moved down", pel_at( hps, 50, 20 ), CLR_RED );
    check( "above the scroll rectangle unchanged", pel_at( hps, 50, 100 ) == CLR_RED, FALSE );
    WinValidateRect( hwnd, NULL, FALSE );

    /* A clip rectangle limits where pels land. */
    WinSetRect( hab, &r, 0, 0, 60, 60 );
    check( "WinScrollWindow clipped",
           WinScrollWindow( hwnd, 0, 5, NULL, &r, NULLHANDLE, &upd, 0 ), RGN_RECT );
    check( "uncovered inside the clip", is_rect( &upd, 0, 0, 60, 5 ), 1 );
    check( "moved inside the clip", pel_at( hps, 50, 44 ), CLR_RED );
    check( "outside the clip unchanged", pel_at( hps, 61, 20 ), CLR_RED );

    WinQueryWindowPos( hwndKid, &swp );
    WinScrollWindow( hwnd, 10, 0, NULL, NULL, NULLHANDLE, NULL, SW_SCROLLCHILDREN );
    {
        LONG x0 = swp.x;
        WinQueryWindowPos( hwndKid, &swp );
        check( "SW_SCROLLCHILDREN moves the child", swp.x - x0, 10 );
    }
    check( "no scroll: RGN_NULL", WinScrollWindow( hwnd, 0, 0, NULL, NULL, NULLHANDLE, &upd, 0 ), RGN_NULL );
    check( "bad option", WinScrollWindow( hwnd, 1, 0, NULL, NULL, NULLHANDLE, NULL, 0x100 ), RGN_ERROR );
    check( "-> PMERR_INVALID_FLAG", last_error(), PMERR_INVALID_FLAG );
    WinReleasePS( hps );
}

static void border_checks( HWND hwnd )
{
    HPS   hps = WinGetPS( hwnd );
    RECTL r;

    WinSetRect( hab, &r, 300, 20, 340, 60 );
    check( "WinDrawBorder", WinDrawBorder( hps, &r, 3, 2, CLR_BLUE, CLR_YELLOW, DB_PATCOPY | DB_INTERIOR ), TRUE );
    check( "border bottom-left", pel_at( hps, 300, 20 ), CLR_BLUE );
    check( "border inside top-right", pel_at( hps, 339, 59 ), CLR_BLUE );
    check( "top-right edge outside", pel_at( hps, 340, 60 ) == CLR_BLUE, FALSE );
    check( "left side 3 wide", pel_at( hps, 302, 40 ), CLR_BLUE );
    check( "interior after the left side", pel_at( hps, 303, 40 ), CLR_YELLOW );
    check( "bottom side 2 high", pel_at( hps, 320, 21 ), CLR_BLUE );
    check( "interior above the bottom side", pel_at( hps, 320, 22 ), CLR_YELLOW );

    check( "DB_DESTINVERT", WinDrawBorder( hps, &r, 3, 2, 0, 0, DB_DESTINVERT ), TRUE );
    check( "inverted border (blue -> yellow)", pel_at( hps, 300, 20 ), CLR_YELLOW );
    check( "interior left alone", pel_at( hps, 320, 40 ), CLR_YELLOW );

    check( "DB_STANDARD", WinDrawBorder( hps, &r, 1, 1, CLR_RED, CLR_GREEN, DB_STANDARD | DB_INTERIOR ), TRUE );
    check( "standard border", pel_at( hps, 300, 40 ), CLR_RED );
    check( "standard interior", pel_at( hps, 301, 40 ), CLR_GREEN );

    check( "invalid flag", WinDrawBorder( hps, &r, 1, 1, 0, 0, 0x4000 ), FALSE );
    check( "-> PMERR_INVALID_FLAG", last_error(), PMERR_INVALID_FLAG );
    check( "invalid raster operation", WinDrawBorder( hps, &r, 1, 1, 0, 0, 5 ), FALSE );
    check( "-> PMERR_INV_DRAW_BORDER_OPTION", last_error(), PMERR_INV_DRAW_BORDER_OPTION );
    WinReleasePS( hps );
}

/* ---------- the picture ---------- */

static void draw_picture( HPS hps, HWND hwnd, PRECTL prcl )
{
    RECTL line, r;
    char  text[80];
    int   i, col;
    LONG  x0 = prcl->xRight - 260;

    WinFillRect( hps, prcl, CLR_WHITE );
    WinSetRect( hab, &line, 8, prcl->yTop - LINE_H, x0 - 8, prcl->yTop - 1 );
    strcpy( text, "passed " );
    append_long( text + strlen( text ), n_passed );
    strcat( text, "/" );
    append_long( text + strlen( text ), n_checks );
    WinDrawText( hps, -1, text, &line, CLR_DARKBLUE, CLR_WHITE, DT_LEFT | DT_TOP | DT_UNDERSCORE );
    /* The failed checks, if any. */
    for( i = 0; i < n_checks; i++ ) {
        if( strstr( results[i], ": FAIL" ) == NULL ) continue;
        WinOffsetRect( hab, &line, 0, -LINE_H );
        if( line.yBottom < 0 ) break;
        WinDrawText( hps, -1, results[i], &line, CLR_RED, CLR_WHITE, DT_LEFT | DT_TOP );
    }

    /* Borders: plain with interior, standard, dialog, inverted. */
    WinSetRect( hab, &r, x0, prcl->yTop - 70, x0 + 100, prcl->yTop - 10 );
    WinDrawBorder( hps, &r, 6, 4, CLR_DARKGREEN, CLR_YELLOW, DB_PATCOPY | DB_INTERIOR );
    WinSetRect( hab, &r, x0 + 120, prcl->yTop - 70, x0 + 220, prcl->yTop - 10 );
    WinDrawBorder( hps, &r, 4, 4, CLR_BLACK, CLR_PALEGRAY, DB_STANDARD | DB_INTERIOR );
    WinSetRect( hab, &r, x0, prcl->yTop - 150, x0 + 100, prcl->yTop - 90 );
    WinDrawBorder( hps, &r, 5, 5, 0, CLR_CYAN, DB_DLGBORDER | DB_INTERIOR );
    WinSetRect( hab, &r, x0 + 120, prcl->yTop - 150, x0 + 220, prcl->yTop - 90 );
    WinFillRect( hps, &r, CLR_RED );
    WinDrawBorder( hps, &r, 10, 10, 0, 0, DB_DESTINVERT );

    /* Stripes, then the lower half scrolled right by 20 within its box. */
    for( col = 0; col < 8; col++ ) {
        WinSetRect( hab, &r, x0 + col * 25, 20, x0 + col * 25 + 25, 140 );
        WinFillRect( hps, &r, col % 2 ? CLR_BLUE : CLR_PINK );
    }
    WinSetRect( hab, &r, x0, 20, x0 + 200, 80 );
    WinScrollWindow( hwnd, 20, 0, &r, NULL, NULLHANDLE, NULL, 0 );
    WinSetRect( hab, &r, x0, 20, x0 + 20, 80 );
    WinFillRect( hps, &r, CLR_DARKGRAY );
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_CREATE:
        hwndKid = WinCreateWindow( hwnd, WC_STATIC, "child", WS_VISIBLE | SS_TEXT,
                                   10, 200, 60, 20, hwnd, HWND_TOP, ID_KID, NULL, NULL );
        return 0;
    case WM_PAINT:
    {
        RECTL rcl, win;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &rcl );
        WinQueryWindowRect( hwnd, &win );
        if( step == 0 ) {
            check( "first WM_PAINT: the whole window", is_rect( &rcl, 0, 0, win.xRight, win.yTop ), 1 );
            WinFillRect( hps, &win, CLR_WHITE );
            step = 1;
            WinPostMsg( hwnd, WM_USER, 0, 0 );
        } else if( step == 2 ) {
            char line[64];
            check( "WinBeginPaint reports the update rectangle", is_rect( &rcl, 5, 5, 25, 25 ), 1 );
            step = 3;
            strcpy( line, PREFIX "done " );
            append_long( line + strlen( line ), n_passed );
            strcat( line, "/" );
            append_long( line + strlen( line ), n_checks );
            strcat( line, "\r\n" );
            write_stdout( line );
            WinEndPaint( hps );
            WinInvalidateRect( hwnd, NULL, FALSE );
            return 0;
        } else if( step == 3 ) {
            draw_picture( hps, hwnd, &win );
        }
        WinEndPaint( hps );
        return 0;
    }
    case WM_USER:
    {
        RECTL r;
        update_checks( hwnd );
        scroll_checks( hwnd );
        border_checks( hwnd );
        WinValidateRect( hwnd, NULL, TRUE );
        step = 2;
        WinSetRect( hab, &r, 5, 5, 25, 25 );
        WinInvalidateRect( hwnd, &r, FALSE );
        return 0;
    }
    case WM_CLOSE:
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    HWND  hwndFrame, hwndClient;
    QMSG  qmsg;
    ULONG flFrameFlags = FCF_TITLEBAR | FCF_SIZEBORDER | FCF_MINMAX |
                         FCF_SYSMENU | FCF_TASKLIST;

    hab = WinInitialize( 0 );
    if( hab == 0 ) return 1;
    hmq = WinCreateMsgQueue( hab, 0 );
    if( hmq == 0 ) {
        WinTerminate( hab );
        return 1;
    }

    WinRegisterClass( hab, "PMPaintTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flFrameFlags,
                                    "PMPaintTest", "Update rectangles, WinScrollWindow, WinDrawBorder",
                                    0, NULLHANDLE, 0, &hwndClient );
    if( hwndFrame == 0 ) {
        WinDestroyMsgQueue( hmq );
        WinTerminate( hab );
        return 1;
    }

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( hwndFrame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
