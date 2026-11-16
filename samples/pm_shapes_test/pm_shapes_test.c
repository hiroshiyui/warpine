/*
 * pm_shapes_test.c - GPI arcs, area patterns and line widths for Warpine
 *
 * On its first WM_PAINT the window draws arcs (GpiFullArc, GpiPartialArc,
 * GpiPointArc, GpiSetArcParams), patterned fills and blits (GpiSetPattern,
 * GpiSetPatternRefPoint, ROP_PATCOPY) and wide lines (GpiSetLineWidth,
 * GpiSetLineWidthGeom), and reads the results back with GpiQueryPel and the
 * query functions. Each check is shown in the window and written to stdout:
 *
 *   pm_shapes_test: <name>: ok
 *   pm_shapes_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_shapes_test: done <passed>/<total>". The right half of the
 * window then shows the same primitives as a picture.
 *
 * Only default colour-table indices are compared, and pels are chosen on
 * the curve's axes (exact for circles) or well inside / outside a shape.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSFILEMGR
#include <os2.h>
#include <string.h>

/* The Open Watcom <pmgpi.h> has no prototype for this export (PMGPI.422);
 * without one the compiler would use its own calling convention. */
BOOL APIENTRY GpiQueryArcParams( HPS hps, PARCPARAMS parcpArcParams );

#define N_CHECKS_MAX 32
#define LINE_H       16
#define COLUMN_ROWS  16    /* result lines in the first column */
#define PICTURE_X    340   /* left edge of the picture and second column */
#define PICTURE_H    226   /* height of the picture */

static HAB   hab;
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
    strcpy( line, "pm_shapes_test: " );
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

static void box( HPS hps, LONG control, LONG x1, LONG y1, LONG x2, LONG y2 )
{
    POINTL pt;
    move_to( hps, x1, y1 );
    pt.x = x2;
    pt.y = y2;
    GpiBox( hps, control, &pt, 0, 0 );
}

static void line_to( HPS hps, LONG x, LONG y )
{
    POINTL pt;
    pt.x = x;
    pt.y = y;
    GpiLine( hps, &pt );
}

static void set_arc_params( HPS hps, LONG p, LONG q, LONG r, LONG s )
{
    ARCPARAMS ap;
    ap.lP = p; ap.lQ = q; ap.lR = r; ap.lS = s;
    GpiSetArcParams( hps, &ap );
}

/* ---------- the checks ---------- */

static void arc_checks( HPS hps )
{
    ARCPARAMS ap;
    POINTL    pt, apt[2];

    GpiErase( hps );
    GpiSetColor( hps, CLR_RED );

    memset( &ap, 0, sizeof( ap ) );
    GpiQueryArcParams( hps, &ap );
    check( "default arc params are a circle", ap.lP == 1 && ap.lQ == 1 && ap.lR == 0 && ap.lS == 0, TRUE );

    move_to( hps, 60, 60 );
    check( "GpiFullArc outline", GpiFullArc( hps, DRO_OUTLINE, MAKEFIXED( 20, 0 ) ), GPI_OK );
    check( "circle passes (80,60)", pel_at( hps, 80, 60 ), CLR_RED );
    check( "circle passes (60,40)", pel_at( hps, 60, 40 ), CLR_RED );
    check( "outline leaves the centre", pel_at( hps, 60, 60 ), CLR_BACKGROUND );
    GpiQueryCurrentPosition( hps, &pt );
    check( "GpiFullArc keeps the position", pt.x == 60 && pt.y == 60, TRUE );

    move_to( hps, 130, 60 );
    GpiFullArc( hps, DRO_FILL, MAKEFIXED( 10, 0 ) );
    check( "filled arc covers the centre", pel_at( hps, 130, 60 ), CLR_RED );
    check( "GpiFullArc rejects control 0", GpiFullArc( hps, 0, MAKEFIXED( 10, 0 ) ), GPI_ERROR );

    set_arc_params( hps, 2, 1, 0, 0 );
    memset( &ap, 0, sizeof( ap ) );
    GpiQueryArcParams( hps, &ap );
    check( "GpiQueryArcParams after set", ap.lP == 2 && ap.lQ == 1, TRUE );
    move_to( hps, 230, 60 );
    GpiFullArc( hps, DRO_OUTLINE, MAKEFIXED( 20, 0 ) );
    check( "ellipse is 2x wider", pel_at( hps, 270, 60 ), CLR_RED );
    check( "and 1x high", pel_at( hps, 230, 80 ), CLR_RED );
    set_arc_params( hps, 1, 1, 0, 0 );

    /* Partial arc: centre (60,140), radius 20, 0..90 degrees. */
    move_to( hps, 20, 140 );
    pt.x = 60; pt.y = 140;
    GpiPartialArc( hps, &pt, MAKEFIXED( 20, 0 ), MAKEFIXED( 0, 0 ), MAKEFIXED( 90, 0 ) );
    GpiQueryCurrentPosition( hps, &pt );
    check( "GpiPartialArc ends at 90 degrees", pt.x == 60 && pt.y == 160, TRUE );
    check( "partial arc starts at 0 degrees", pel_at( hps, 80, 140 ), CLR_RED );
    check( "270 degrees not swept", pel_at( hps, 60, 120 ), CLR_BACKGROUND );

    /* Point arc through (170,140) -> (150,160) -> (130,140). */
    move_to( hps, 170, 140 );
    apt[0].x = 150; apt[0].y = 160;
    apt[1].x = 130; apt[1].y = 140;
    GpiPointArc( hps, apt );
    check( "GpiPointArc through the middle point", pel_at( hps, 150, 160 ), CLR_RED );
    GpiQueryCurrentPosition( hps, &pt );
    check( "GpiPointArc ends at the last point", pt.x == 130 && pt.y == 140, TRUE );
}

static void pattern_checks( HPS hps )
{
    POINTL pt, apt[2];

    GpiErase( hps );
    GpiSetColor( hps, CLR_RED );
    check( "default pattern", GpiQueryPattern( hps ), PATSYM_DEFAULT );
    GpiSetPattern( hps, PATSYM_HALFTONE );
    check( "GpiQueryPattern", GpiQueryPattern( hps ), PATSYM_HALFTONE );
    check( "GpiSetPattern rejects 20", GpiSetPattern( hps, 20 ), FALSE );

    /* Halftone from the origin: (0,0) set, (1,0) clear. */
    box( hps, DRO_FILL, 16, 16, 31, 31 );
    check( "halftone set pel", pel_at( hps, 16, 16 ), CLR_RED );
    check( "halftone clear pel left alone", pel_at( hps, 17, 16 ), CLR_BACKGROUND );

    pt.x = 1; pt.y = 0;
    GpiSetPatternRefPoint( hps, &pt );
    pt.x = pt.y = 0;
    GpiQueryPatternRefPoint( hps, &pt );
    check( "GpiQueryPatternRefPoint", pt.x, 1 );

    /* ROP_PATCOPY: clear pels take the background colour. */
    GpiSetBackColor( hps, CLR_BLUE );
    apt[0].x = 40; apt[0].y = 16;
    apt[1].x = 56; apt[1].y = 32;
    GpiBitBlt( hps, NULLHANDLE, 2, apt, ROP_PATCOPY, BBO_IGNORE );
    check( "ROP_PATCOPY set pel (shifted)", pel_at( hps, 41, 16 ), CLR_RED );
    check( "ROP_PATCOPY clear pel", pel_at( hps, 40, 16 ), CLR_BLUE );
    pt.x = pt.y = 0;
    GpiSetPatternRefPoint( hps, &pt );
    GpiSetPattern( hps, PATSYM_DEFAULT );
    GpiSetBackColor( hps, CLR_BACKGROUND );
}

static void width_checks( HPS hps )
{
    LONG y, rows = 0;

    GpiErase( hps );
    GpiSetColor( hps, CLR_RED );
    check( "default line width", GpiQueryLineWidth( hps ), LINEWIDTH_DEFAULT );
    GpiSetLineWidth( hps, LINEWIDTH_THICK );
    check( "GpiQueryLineWidth", GpiQueryLineWidth( hps ), LINEWIDTH_THICK );
    move_to( hps, 20, 50 );
    line_to( hps, 120, 50 );
    for( y = 45; y <= 55; y++ ) {
        if( pel_at( hps, 70, y ) == CLR_RED ) rows++;
    }
    check( "thick line is 2 pels wide", rows, 2 );
    check( "GpiSetLineWidth rejects -1", GpiSetLineWidth( hps, MAKEFIXED( -1, 0 ) ), FALSE );
    GpiSetLineWidth( hps, LINEWIDTH_NORMAL );

    GpiSetLineWidthGeom( hps, 10 );
    check( "GpiQueryLineWidthGeom", GpiQueryLineWidthGeom( hps ), 10 );
}

static void run_checks( HPS hps )
{
    arc_checks( hps );
    pattern_checks( hps );
    width_checks( hps );
}

/* ---------- the picture (right half) ---------- */

static void draw_picture( HPS hps, PRECTL prcl )
{
    static const LONG patterns[8] = {
        PATSYM_DENSE2, PATSYM_DENSE6, PATSYM_VERT, PATSYM_HORIZ,
        PATSYM_DIAG1, PATSYM_DIAG3, PATSYM_HATCH, PATSYM_DIAGHATCH
    };
    LONG   x0 = PICTURE_X, top = prcl->yTop, i;
    POINTL pt;

    /* Circle, filled ellipse, pie-style partial arc. */
    GpiSetColor( hps, CLR_DARKBLUE );
    move_to( hps, x0 + 40, top - 50 );
    GpiFullArc( hps, DRO_OUTLINE, MAKEFIXED( 30, 0 ) );
    set_arc_params( hps, 2, 1, 0, 0 );
    GpiSetColor( hps, CLR_DARKGREEN );
    move_to( hps, x0 + 170, top - 50 );
    GpiFullArc( hps, DRO_OUTLINEFILL, MAKEFIXED( 25, 0 ) );
    set_arc_params( hps, 1, 1, 0, 0 );
    GpiSetColor( hps, CLR_RED );
    move_to( hps, x0 + 250, top - 50 );
    pt.x = x0 + 250; pt.y = top - 50;
    GpiPartialArc( hps, &pt, MAKEFIXED( 30, 0 ), MAKEFIXED( 30, 0 ), MAKEFIXED( 240, 0 ) );
    line_to( hps, x0 + 250, top - 50 );

    /* Pattern swatches. */
    GpiSetColor( hps, CLR_BLACK );
    for( i = 0; i < 8; i++ ) {
        GpiSetPattern( hps, patterns[i] );
        box( hps, DRO_OUTLINEFILL, x0 + i * 34, top - 130, x0 + i * 34 + 28, top - 100 );
    }
    GpiSetPattern( hps, PATSYM_DEFAULT );

    /* Line widths 1, 2, 4, 8. */
    GpiSetColor( hps, CLR_DARKRED );
    for( i = 0; i < 4; i++ ) {
        GpiSetLineWidth( hps, MAKEFIXED( 1 << i, 0 ) );
        move_to( hps, x0, top - 160 - i * 18 );
        line_to( hps, x0 + 260, top - 160 - i * 18 );
    }
    GpiSetLineWidth( hps, LINEWIDTH_NORMAL );
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
            run_checks( hps );
            strcpy( line, "pm_shapes_test: done " );
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

    WinRegisterClass( hab, "PMShapesTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flFrameFlags,
                                    "PMShapesTest", "Warpine GPI shapes test",
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
