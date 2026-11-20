/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_rect_text_test.c - rectangle helpers, WinDrawText and GpiPolyLine
 *
 * On its first WM_PAINT the window runs its checks and writes each one to
 * stdout:
 *
 *   pm_rect_text_test: <name>: ok
 *   pm_rect_text_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_rect_text_test: done <passed>/<total>". The window then
 * shows the results, drawn with WinDrawText, and a polyline.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSFILEMGR
#include <os2.h>
#include <string.h>

#define N_CHECKS_MAX 48
#define LINE_H       16
#define PREFIX       "pm_rect_text_test: "

static HAB   hab;
static BOOL  checked = FALSE;
static int   n_checks = 0;
static int   n_passed = 0;
static char  results[N_CHECKS_MAX][80];

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

/* ---------- the checks ---------- */

static void rect_checks( void )
{
    RECTL  a, b, d;
    POINTL pt;

    check( "WinSetRect", WinSetRect( hab, &a, 10, 20, 30, 40 ), TRUE );
    check( "WinSetRect fields", is_rect( &a, 10, 20, 30, 40 ), 1 );
    check( "WinIsRectEmpty (area)", WinIsRectEmpty( hab, &a ), FALSE );
    pt.x = 10; pt.y = 20;
    check( "WinPtInRect bottom-left corner", WinPtInRect( hab, &a, &pt ), TRUE );
    pt.x = 30;
    check( "WinPtInRect right edge", WinPtInRect( hab, &a, &pt ), FALSE );

    WinCopyRect( hab, &b, &a );
    check( "WinCopyRect", WinEqualRect( hab, &a, &b ), TRUE );
    WinOffsetRect( hab, &b, 15, -10 );
    check( "WinOffsetRect", is_rect( &b, 25, 10, 45, 30 ), 1 );
    check( "WinIntersectRect", WinIntersectRect( hab, &d, &a, &b ), TRUE );
    check( "intersection", is_rect( &d, 25, 20, 30, 30 ), 1 );
    check( "WinUnionRect", WinUnionRect( hab, &d, &a, &b ), TRUE );
    check( "union", is_rect( &d, 10, 10, 45, 40 ), 1 );

    WinOffsetRect( hab, &b, 100, 0 );
    check( "WinIntersectRect disjoint", WinIntersectRect( hab, &d, &a, &b ), FALSE );
    check( "WinIsRectEmpty (no intersection)", WinIsRectEmpty( hab, &d ), TRUE );

    WinSetRect( hab, &b, 0, 0, 99, 25 );
    check( "WinSubtractRect band", WinSubtractRect( hab, &d, &a, &b ), TRUE );
    check( "difference", is_rect( &d, 10, 25, 30, 40 ), 1 );
    WinSetRect( hab, &b, 15, 25, 20, 30 );
    WinSubtractRect( hab, &d, &a, &b );
    check( "WinSubtractRect hole gives the first", is_rect( &d, 10, 20, 30, 40 ), 1 );

    WinCopyRect( hab, &b, &a );
    WinInflateRect( hab, &b, 2, -3 );
    check( "WinInflateRect", is_rect( &b, 8, 23, 32, 37 ), 1 );

    WinSetRect( hab, &b, 5, 5, 5, 9 );
    WinSetRectEmpty( hab, &d );
    check( "WinEqualRect two empty", WinEqualRect( hab, &b, &d ), TRUE );
    check( "WinSetRectEmpty", is_rect( &d, 0, 0, 0, 0 ), 1 );
}

static void draw_text_checks( HPS hps )
{
    RECTL rcl;
    LONG  n;

    WinSetRect( hab, &rcl, 0, 0, 299, 99 );
    check( "WinDrawText count", WinDrawText( hps, -1, "Hello", &rcl, CLR_BLACK, CLR_WHITE, DT_LEFT ), 5 );
    check( "WinDrawText full length past a line end",
           WinDrawText( hps, 6, "ab\r\ncd", &rcl, CLR_BLACK, CLR_WHITE, DT_LEFT ), 6 );
    check( "WinDrawText invalid flag",
           WinDrawText( hps, -1, "x", &rcl, CLR_BLACK, CLR_WHITE, 0x0001 ), 0 );

    WinSetRect( hab, &rcl, 20, 10, 299, 99 );
    check( "DT_QUERYEXTENT count",
           WinDrawText( hps, -1, "Warp", &rcl, CLR_BLACK, CLR_WHITE, DT_LEFT | DT_BOTTOM | DT_QUERYEXTENT ), 4 );
    check( "extent left", rcl.xLeft, 20 );
    check( "extent bottom", rcl.yBottom, 10 );
    check( "extent has width", rcl.xRight > rcl.xLeft, TRUE );
    check( "extent has height", rcl.yTop > rcl.yBottom, TRUE );

    /* A rectangle a short word wide: only the first word is drawn. */
    WinSetRect( hab, &rcl, 0, 0, 30, 99 );
    n = WinDrawText( hps, -1, "aa bb cc dd", &rcl, CLR_BLACK, CLR_WHITE, DT_LEFT | DT_WORDBREAK );
    check( "DT_WORDBREAK draws a part", n > 0 && n < 11, TRUE );
    n = WinDrawText( hps, -1, "aa bb", &rcl, CLR_BLACK, CLR_WHITE, DT_LEFT | DT_WORDBREAK );
    check( "DT_WORDBREAK first word at least", n >= 2, TRUE );

    /* DT_ERASERECT fills the rectangle with the background colour. */
    WinSetRect( hab, &rcl, 200, 150, 260, 190 );
    WinDrawText( hps, -1, "E", &rcl, CLR_BLACK, CLR_YELLOW, DT_CENTER | DT_VCENTER | DT_ERASERECT );
    check( "DT_ERASERECT corner", pel_at( hps, 200, 150 ), CLR_YELLOW );
    check( "DT_ERASERECT boundary inside", pel_at( hps, 260, 190 ), CLR_YELLOW );
}

static void poly_line_checks( HPS hps )
{
    POINTL pt, pts[3];

    GpiSetColor( hps, CLR_GREEN );
    pt.x = 300; pt.y = 150;
    GpiMove( hps, &pt );
    pts[0].x = 360; pts[0].y = 150;
    pts[1].x = 360; pts[1].y = 210;
    pts[2].x = 300; pts[2].y = 210;
    check( "GpiPolyLine", GpiPolyLine( hps, 3, pts ), GPI_OK );
    check( "first side from the current position", pel_at( hps, 330, 150 ), CLR_GREEN );
    check( "second side", pel_at( hps, 360, 180 ), CLR_GREEN );
    check( "third side", pel_at( hps, 330, 210 ), CLR_GREEN );
    check( "not closed", pel_at( hps, 300, 180 ) == CLR_GREEN, FALSE );
    GpiQueryCurrentPosition( hps, &pt );
    check( "position at the last point", pt.x == 300 && pt.y == 210, TRUE );
    check( "GpiPolyLine zero points", GpiPolyLine( hps, 0, pts ), GPI_OK );
    check( "GpiPolyLine negative count", GpiPolyLine( hps, -1, pts ), GPI_ERROR );
}

static void draw_results( HPS hps, PRECTL prcl )
{
    RECTL  line;
    POINTL pt, pts[4];
    char   text[80];
    int    i;

    WinFillRect( hps, prcl, CLR_WHITE );
    WinSetRect( hab, &line, 8, prcl->yTop - LINE_H, prcl->xRight - 8, prcl->yTop - 1 );
    for( i = 0; i < n_checks; i++ ) {
        WinDrawText( hps, -1, results[i], &line, CLR_BLACK, CLR_WHITE, DT_LEFT | DT_TOP );
        WinOffsetRect( hab, &line, 0, -LINE_H );
    }
    strcpy( text, "passed " );
    append_long( text + strlen( text ), n_passed );
    strcat( text, "/" );
    append_long( text + strlen( text ), n_checks );
    WinDrawText( hps, -1, text, &line, CLR_DARKBLUE, CLR_WHITE, DT_LEFT | DT_TOP | DT_UNDERSCORE );

    /* A star drawn as one polyline. */
    GpiSetColor( hps, CLR_RED );
    pt.x = prcl->xRight - 100; pt.y = 40;
    GpiMove( hps, &pt );
    pts[0].x = pt.x + 30; pts[0].y = 120;
    pts[1].x = pt.x + 60; pts[1].y = 40;
    pts[2].x = pt.x - 10; pts[2].y = 90;
    pts[3].x = pt.x + 70; pts[3].y = 90;
    GpiPolyLine( hps, 4, pts );
    GpiLine( hps, &pt );
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PAINT:
    {
        RECTL rcl;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        WinQueryWindowRect( hwnd, &rcl );
        if( !checked ) {
            char line[64];
            checked = TRUE;
            WinFillRect( hps, &rcl, CLR_WHITE );
            rect_checks();
            draw_text_checks( hps );
            poly_line_checks( hps );
            strcpy( line, PREFIX "done " );
            append_long( line + strlen( line ), n_passed );
            strcat( line, "/" );
            append_long( line + strlen( line ), n_checks );
            strcat( line, "\r\n" );
            write_stdout( line );
        }
        draw_results( hps, &rcl );
        WinEndPaint( hps );
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

    WinRegisterClass( hab, "PMRectTextTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flFrameFlags,
                                    "PMRectTextTest", "Rectangles, WinDrawText, GpiPolyLine",
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
