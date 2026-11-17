/*
 * pm_region_test.c - GPI regions, clipping, areas, paths, polygons and
 * curves for Warpine
 *
 * On its first WM_PAINT the window creates and combines regions
 * (GpiCreateRegion, GpiCombineRegion, GpiQueryRegionBox,
 * GpiQueryRegionRects, GpiPtInRegion, GpiRectInRegion, GpiOffsetRegion,
 * GpiEqualRegion), paints them (GpiPaintRegion, GpiFrameRegion), clips
 * drawing (GpiIntersectClipRectangle, GpiExcludeClipRectangle,
 * GpiSetClipRegion, GpiQueryClipBox, GpiSetClipPath), draws areas
 * (GpiBeginArea / GpiEndArea, alternate and winding), paths (GpiFillPath,
 * GpiStrokePath, GpiOutlinePath, GpiPathToRegion), polygons (GpiPolygons)
 * and curves (GpiPolySpline, GpiPolyFillet), and reads the results back
 * with GpiQueryPel and the query functions. Each check is shown in the
 * window and written to stdout:
 *
 *   pm_region_test: <name>: ok
 *   pm_region_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_region_test: done <passed>/<total>". The right half of
 * the window then shows the same features as a picture.
 *
 * Only default colour-table indices are compared, and pels are chosen well
 * inside or outside a shape, or on edges whose inclusion the PM reference
 * states.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSFILEMGR
#include <os2.h>
#include <string.h>

#define N_CHECKS_MAX 64
#define LINE_H       16
#define COLUMN_ROWS  34    /* result lines in the first column */
#define PICTURE_X    340   /* left edge of the picture and second column */
#define PICTURE_H    240   /* height of the picture */

static HAB   hab;
static HWND  client;
static BOOL  checked = FALSE;
static int   n_checks = 0;
static int   n_passed = 0;
static char  results[N_CHECKS_MAX][64];

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
    strcpy( line, "pm_region_test: " );
    strcat( line, results[n_checks] );
    strcat( line, "\r\n" );
    write_stdout( line );
    n_checks++;
}

static LONG pel_at( HPS hps, LONG x, LONG y )
{
    POINTL pt;
    pt.x = x;
    pt.y = y;
    return GpiQueryPel( hps, &pt );
}

static void move_to( HPS hps, LONG x, LONG y )
{
    POINTL pt;
    pt.x = x;
    pt.y = y;
    GpiMove( hps, &pt );
}

static void line_to( HPS hps, LONG x, LONG y )
{
    POINTL pt;
    pt.x = x;
    pt.y = y;
    GpiLine( hps, &pt );
}

static void box( HPS hps, LONG control, LONG x1, LONG y1, LONG x2, LONG y2 )
{
    POINTL pt;
    move_to( hps, x1, y1 );
    pt.x = x2;
    pt.y = y2;
    GpiBox( hps, control, &pt, 0, 0 );
}

static void set_rect( PRECTL r, LONG l, LONG b, LONG rt, LONG t )
{
    r->xLeft = l; r->yBottom = b; r->xRight = rt; r->yTop = t;
}

/* A square (x, y) - (x + s, y + s) as one figure, counter-clockwise. */
static void square( HPS hps, LONG x, LONG y, LONG s )
{
    POINTL apt[3];
    move_to( hps, x, y );
    apt[0].x = x + s; apt[0].y = y;
    apt[1].x = x + s; apt[1].y = y + s;
    apt[2].x = x;     apt[2].y = y + s;
    GpiPolyLine( hps, 3, apt );
}

/* The triangle (20, 10) - (60, 10) - (40, 50) as a path. */
static void triangle_path( HPS hps )
{
    POINTL apt[2];
    GpiBeginPath( hps, 1 );
    move_to( hps, 20, 10 );
    apt[0].x = 60; apt[0].y = 10;
    apt[1].x = 40; apt[1].y = 50;
    GpiPolyLine( hps, 2, apt );
    GpiCloseFigure( hps );
    GpiEndPath( hps );
}

/* ---------- the checks ---------- */

static void region_checks( HPS hps )
{
    RECTL   arc[3], rc;
    POINTL  pt;
    RGNRECT ctl;
    HRGN    a, b, d;

    set_rect( &arc[0], 10, 10, 20, 20 );
    set_rect( &arc[1], 15, 15, 30, 25 );
    a = GpiCreateRegion( hps, 2, arc );
    check( "create", a != NULLHANDLE, 1 );
    check( "box complexity", GpiQueryRegionBox( hps, a, &rc ), RGN_COMPLEX );
    check( "box right (exclusive)", rc.xRight, 30 );
    pt.x = 10; pt.y = 10;
    check( "pt bottom-left in", GpiPtInRegion( hps, a, &pt ), PRGN_INSIDE );
    pt.x = 20; pt.y = 12;
    check( "pt right edge out", GpiPtInRegion( hps, a, &pt ), PRGN_OUTSIDE );
    set_rect( &rc, 0, 0, 12, 12 );
    check( "rect partial", GpiRectInRegion( hps, a, &rc ), RRGN_PARTIAL );

    set_rect( &arc[0], 0, 0, 10, 10 );
    set_rect( &arc[1], 5, 0, 15, 10 );
    a = GpiCreateRegion( hps, 1, &arc[0] );
    b = GpiCreateRegion( hps, 1, &arc[1] );
    d = GpiCreateRegion( hps, 0, NULL );
    check( "combine xor", GpiCombineRegion( hps, d, a, b, CRGN_XOR ), RGN_COMPLEX );
    check( "combine and", GpiCombineRegion( hps, d, a, b, CRGN_AND ), RGN_RECT );
    pt.x = -5; pt.y = 0;
    GpiOffsetRegion( hps, b, &pt );
    check( "offset equal", GpiEqualRegion( hps, a, b ), EQRGN_EQUAL );

    set_rect( &arc[0], 0, 0, 2, 2 );
    set_rect( &arc[1], 4, 0, 6, 2 );
    set_rect( &arc[2], 0, 4, 2, 6 );
    GpiSetRegion( hps, d, 3, arc );
    ctl.ircStart = 1; ctl.crc = 3; ctl.crcReturned = 0; ctl.ulDirection = RECTDIR_LFRT_TOPBOT;
    GpiQueryRegionRects( hps, d, NULL, &ctl, arc );
    check( "rects returned", ctl.crcReturned, 3 );
    check( "rects top first", arc[0].yBottom, 4 );
    GpiDestroyRegion( hps, a );
    GpiDestroyRegion( hps, b );
    GpiDestroyRegion( hps, d );

    /* Painting and framing. */
    GpiErase( hps );
    GpiSetColor( hps, CLR_RED );
    set_rect( &arc[0], 10, 10, 30, 30 );
    a = GpiCreateRegion( hps, 1, arc );
    GpiPaintRegion( hps, a );
    check( "paint inside", pel_at( hps, 29, 29 ), CLR_RED );
    check( "paint right edge out", pel_at( hps, 30, 20 ), CLR_BACKGROUND );
    GpiSetColor( hps, CLR_BLUE );
    set_rect( &arc[0], 50, 10, 80, 40 );
    GpiSetRegion( hps, a, 1, arc );
    {
        SIZEL th;
        th.cx = 3; th.cy = 2;
        GpiFrameRegion( hps, a, &th );
    }
    check( "frame left", pel_at( hps, 52, 25 ), CLR_BLUE );
    check( "frame inside", pel_at( hps, 53, 25 ), CLR_BACKGROUND );
    check( "frame top", pel_at( hps, 65, 38 ), CLR_BLUE );
    GpiDestroyRegion( hps, a );
}

static void clip_checks( HPS hps )
{
    RECTL rc;
    HRGN  old = NULLHANDLE;

    GpiErase( hps );
    GpiSetColor( hps, CLR_RED );
    set_rect( &rc, 20, 20, 39, 39 );
    check( "intersect clip", GpiIntersectClipRectangle( hps, &rc ), RGN_RECT );
    set_rect( &rc, 25, 25, 29, 29 );
    check( "exclude clip", GpiExcludeClipRectangle( hps, &rc ), RGN_COMPLEX );
    GpiQueryClipBox( hps, &rc );
    check( "clip box right (inclusive)", rc.xRight, 39 );
    box( hps, DRO_FILL, 0, 0, 99, 99 );
    check( "clipped in", pel_at( hps, 39, 39 ), CLR_RED );
    check( "clipped out", pel_at( hps, 40, 30 ), CLR_BACKGROUND );
    check( "clipped hole", pel_at( hps, 27, 27 ), CLR_BACKGROUND );
    check( "clip region busy",
           GpiQueryRegionBox( hps, GpiQueryClipRegion( hps ), &rc ), RGN_ERROR );
    GpiSetClipRegion( hps, NULLHANDLE, &old );
    check( "old clip returned", old != NULLHANDLE, 1 );
    check( "old clip freed", GpiDestroyRegion( hps, old ), TRUE );
    box( hps, DRO_FILL, 0, 0, 99, 99 );
    check( "unclipped again", pel_at( hps, 50, 50 ), CLR_RED );

    /* A clip path: the triangle. */
    GpiErase( hps );
    triangle_path( hps );
    check( "set clip path", GpiSetClipPath( hps, 1, SCP_AND ), TRUE );
    GpiSetColor( hps, CLR_GREEN );
    box( hps, DRO_FILL, 0, 0, 99, 99 );
    check( "clip path in", pel_at( hps, 40, 20 ), CLR_GREEN );
    check( "clip path out", pel_at( hps, 20, 40 ), CLR_BACKGROUND );
    GpiSetClipPath( hps, 0, SCP_RESET );
}

static void area_path_checks( HPS hps )
{
    POLYGON pg[2];
    POINTL  apt1[3], apt2[4], pt;
    RECTL   rc;
    HRGN    r;

    GpiErase( hps );
    GpiSetColor( hps, CLR_RED );
    GpiBeginArea( hps, BA_NOBOUNDARY | BA_ALTERNATE );
    square( hps, 10, 10, 30 );
    square( hps, 25, 25, 30 );
    GpiEndArea( hps );
    check( "area alternate", pel_at( hps, 20, 20 ), CLR_RED );
    check( "area overlap out", pel_at( hps, 30, 30 ), CLR_BACKGROUND );
    GpiBeginArea( hps, BA_NOBOUNDARY | BA_WINDING );
    square( hps, 110, 10, 30 );
    square( hps, 125, 25, 30 );
    GpiEndArea( hps );
    check( "area winding overlap in", pel_at( hps, 130, 30 ), CLR_RED );
    GpiBeginArea( hps, BA_EXCL );
    square( hps, 70, 60, 10 );
    GpiEndArea( hps );
    check( "area excl left in", pel_at( hps, 70, 65 ), CLR_RED );
    check( "area excl right out", pel_at( hps, 80, 65 ), CLR_BACKGROUND );

    GpiErase( hps );
    triangle_path( hps );
    check( "fill path", GpiFillPath( hps, 1, FPATH_ALTERNATE ), GPI_OK );
    check( "filled", pel_at( hps, 40, 20 ), CLR_RED );
    check( "path used up", GpiFillPath( hps, 1, FPATH_ALTERNATE ), GPI_ERROR );
    GpiSetColor( hps, CLR_BLUE );
    triangle_path( hps );
    GpiOutlinePath( hps, 1, 0 );
    check( "outline edge", pel_at( hps, 40, 10 ), CLR_BLUE );
    check( "outline inside kept", pel_at( hps, 40, 20 ), CLR_RED );
    GpiErase( hps );
    GpiSetLineWidthGeom( hps, 6 );
    triangle_path( hps );
    GpiStrokePath( hps, 1, 0 );
    check( "stroke wide", pel_at( hps, 40, 8 ), CLR_BLUE );
    check( "stroke hollow", pel_at( hps, 40, 25 ), CLR_BACKGROUND );
    triangle_path( hps );
    r = GpiPathToRegion( hps, 1, FPATH_ALTERNATE );
    GpiQueryRegionBox( hps, r, &rc );
    check( "path region top", rc.yTop, 51 );
    GpiDestroyRegion( hps, r );

    /* Polygons: the first from the current position. */
    GpiErase( hps );
    GpiSetColor( hps, CLR_GREEN );
    apt1[0].x = 40; apt1[0].y = 10;
    apt1[1].x = 40; apt1[1].y = 40;
    apt1[2].x = 10; apt1[2].y = 40;
    apt2[0].x = 25; apt2[0].y = 25;
    apt2[1].x = 55; apt2[1].y = 25;
    apt2[2].x = 55; apt2[2].y = 55;
    apt2[3].x = 25; apt2[3].y = 55;
    pg[0].ulPoints = 3; pg[0].aPointl = apt1;
    pg[1].ulPoints = 4; pg[1].aPointl = apt2;
    move_to( hps, 10, 10 );
    check( "polygons", GpiPolygons( hps, 2, pg, POLYGON_NOBOUNDARY | POLYGON_WINDING, POLYGON_INCL ), GPI_OK );
    check( "polygons winding", pel_at( hps, 30, 30 ), CLR_GREEN );
    check( "polygons outside", pel_at( hps, 50, 15 ), CLR_BACKGROUND );

    /* Curves. */
    GpiErase( hps );
    GpiSetColor( hps, CLR_RED );
    move_to( hps, 10, 10 );
    apt1[0].x = 10; apt1[0].y = 60;
    apt1[1].x = 60; apt1[1].y = 60;
    apt1[2].x = 60; apt1[2].y = 10;
    check( "spline", GpiPolySpline( hps, 3, apt1 ), GPI_OK );
    GpiQueryCurrentPosition( hps, &pt );
    check( "spline end position", pt.x, 60 );
    check( "spline end pel", pel_at( hps, 60, 10 ), CLR_RED );
    move_to( hps, 100, 10 );
    apt1[0].x = 100; apt1[0].y = 60;
    apt1[1].x = 150; apt1[1].y = 60;
    GpiPolyFillet( hps, 2, apt1 );
    check( "fillet cuts the corner", pel_at( hps, 100, 60 ), CLR_BACKGROUND );
    check( "fillet ends on the point", pel_at( hps, 150, 60 ), CLR_RED );
}

static void window_region_checks( HPS hps )
{
    RECTL rc;
    HRGN  r, q;

    set_rect( &rc, 10, 10, 20, 20 );
    r = GpiCreateRegion( hps, 1, &rc );
    q = GpiCreateRegion( hps, 0, NULL );
    WinValidateRect( client, NULL, FALSE );
    WinInvalidateRegion( client, r, FALSE );
    check( "update region", WinQueryUpdateRegion( client, q ), RGN_RECT );
    GpiQueryRegionBox( hps, q, &rc );
    check( "update region right", rc.xRight, 20 );
    WinValidateRegion( client, r, FALSE );
    check( "validated", WinQueryUpdateRegion( client, q ), RGN_NULL );
    GpiDestroyRegion( hps, r );
    GpiDestroyRegion( hps, q );
}

static void run_checks( HPS hps )
{
    region_checks( hps );
    clip_checks( hps );
    area_path_checks( hps );
    window_region_checks( hps );
    GpiSetLineWidthGeom( hps, 0 );
}

/* ---------- the picture (right half) ---------- */

static void star( HPS hps, LONG cx, LONG cy, LONG options )
{
    static const LONG dx[5] = { 0, 38, -47, 47, -38 };
    static const LONG dy[5] = { 50, -40, 15, 15, -40 };
    POINTL  apt[4];
    POLYGON pg;
    int     i;

    move_to( hps, cx + dx[0], cy + dy[0] );
    for( i = 1; i < 5; i++ ) {
        apt[i - 1].x = cx + dx[i];
        apt[i - 1].y = cy + dy[i];
    }
    pg.ulPoints = 4;
    pg.aPointl = apt;
    GpiPolygons( hps, 1, &pg, POLYGON_BOUNDARY | options, POLYGON_INCL );
}

static void draw_picture( HPS hps, PRECTL prcl )
{
    LONG   x0 = PICTURE_X, top = prcl->yTop, i;
    RECTL  arc[3];
    HRGN   r, old;
    POINTL apt[3];
    SIZEL  th;

    /* Pentagrams: alternate (hollow centre) and winding (filled). */
    GpiSetColor( hps, CLR_DARKBLUE );
    star( hps, x0 + 50, top - 60, POLYGON_ALTERNATE );
    GpiSetColor( hps, CLR_DARKGREEN );
    star( hps, x0 + 160, top - 60, POLYGON_WINDING );

    /* A region painted with a pattern, then framed. */
    set_rect( &arc[0], x0 + 220, top - 110, x0 + 300, top - 80 );
    set_rect( &arc[1], x0 + 240, top - 80, x0 + 280, top - 20 );
    r = GpiCreateRegion( hps, 2, arc );
    GpiSetColor( hps, CLR_RED );
    GpiSetPattern( hps, PATSYM_DIAG1 );
    GpiPaintRegion( hps, r );
    GpiSetPattern( hps, PATSYM_DEFAULT );
    GpiSetColor( hps, CLR_BLACK );
    th.cx = 2; th.cy = 2;
    GpiFrameRegion( hps, r, &th );
    GpiDestroyRegion( hps, r );

    /* Stripes clipped to a circle (a clip path). */
    GpiBeginPath( hps, 1 );
    move_to( hps, x0 + 50, top - 170 );
    GpiFullArc( hps, DRO_OUTLINE, MAKEFIXED( 40, 0 ) );
    GpiEndPath( hps );
    GpiSetClipPath( hps, 1, SCP_AND );
    GpiSetColor( hps, CLR_PINK );
    for( i = 0; i < 100; i += 6 ) {
        move_to( hps, x0 + i, top - 220 );
        line_to( hps, x0 + i + 50, top - 120 );
    }
    GpiSetClipPath( hps, 0, SCP_RESET );

    /* Stripes clipped to a region with a hole. */
    set_rect( &arc[0], x0 + 110, top - 210, x0 + 190, top - 130 );
    GpiIntersectClipRectangle( hps, &arc[0] );
    set_rect( &arc[0], x0 + 135, top - 185, x0 + 165, top - 155 );
    GpiExcludeClipRectangle( hps, &arc[0] );
    GpiSetColor( hps, CLR_DARKCYAN );
    for( i = 0; i < 90; i += 5 ) {
        move_to( hps, x0 + 110, top - 210 + i );
        line_to( hps, x0 + 190, top - 210 + i );
    }
    GpiSetClipRegion( hps, NULLHANDLE, &old );
    GpiDestroyRegion( hps, old );

    /* A stroked zigzag path and a spline. */
    GpiSetLineWidthGeom( hps, 8 );
    GpiSetColor( hps, CLR_BROWN );
    GpiBeginPath( hps, 1 );
    move_to( hps, x0 + 210, top - 200 );
    apt[0].x = x0 + 240; apt[0].y = top - 150;
    apt[1].x = x0 + 270; apt[1].y = top - 200;
    apt[2].x = x0 + 300; apt[2].y = top - 150;
    GpiPolyLine( hps, 3, apt );
    GpiEndPath( hps );
    GpiStrokePath( hps, 1, 0 );
    GpiSetLineWidthGeom( hps, 0 );
    GpiSetColor( hps, CLR_DARKRED );
    move_to( hps, x0 + 210, top - 230 );
    apt[0].x = x0 + 230; apt[0].y = top - 120;
    apt[1].x = x0 + 280; apt[1].y = top - 260;
    apt[2].x = x0 + 300; apt[2].y = top - 130;
    GpiPolySpline( hps, 3, apt );
}

static void draw_results( HPS hps, PRECTL prcl )
{
    POINTL pt;
    char   line[80];
    int    i;

    WinFillRect( hps, prcl, CLR_WHITE );
    GpiSetColor( hps, CLR_BLACK );
    pt.x = 8;
    pt.y = prcl->yTop - LINE_H;
    for( i = 0; i < n_checks; i++ ) {
        if( i == COLUMN_ROWS ) {
            /* Second column, below the picture. */
            pt.x = PICTURE_X;
            pt.y = prcl->yTop - PICTURE_H - LINE_H;
        }
        GpiCharStringAt( hps, &pt, strlen( results[i] ), results[i] );
        pt.y -= LINE_H;
        if( i >= COLUMN_ROWS && pt.y < 0 ) break;
    }
    strcpy( line, "passed " );
    append_long( line + strlen( line ), n_passed );
    strcat( line, "/" );
    append_long( line + strlen( line ), n_checks );
    GpiCharStringAt( hps, &pt, strlen( line ), line );
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
            client = hwnd;
            run_checks( hps );
            strcpy( line, "pm_region_test: done " );
            append_long( line + strlen( line ), n_passed );
            strcat( line, "/" );
            append_long( line + strlen( line ), n_checks );
            strcat( line, "\r\n" );
            write_stdout( line );
        }
        draw_results( hps, &rcl );
        draw_picture( hps, &rcl );
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

    WinRegisterClass( hab, "PMRegionTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flFrameFlags,
                                    "PMRegionTest", "Warpine GPI regions test",
                                    0, NULLHANDLE, 0, &hwndClient );
    if( hwndFrame == 0 ) {
        WinDestroyMsgQueue( hmq );
        WinTerminate( hab );
        return 1;
    }

    /* Room for the results and the picture. */
    WinSetWindowPos( hwndFrame, NULLHANDLE, 20, 60, 680, 640, SWP_MOVE | SWP_SIZE );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( hwndFrame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
