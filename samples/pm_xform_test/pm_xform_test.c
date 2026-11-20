/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_xform_test.c - GPI presentation-space state and transforms for Warpine
 *
 * On its first WM_PAINT the window checks the coordinate spaces of a
 * presentation space: model transforms built with GpiTranslate, GpiScale
 * and GpiRotate (GpiSetModelTransformMatrix, TRANSFORM_REPLACE / ADD),
 * GpiConvert and GpiConvertWithMatrix, the page viewport, page units of a
 * GpiCreatePS presentation space (PU_LOENGLISH), clip rectangles in world
 * coordinates, viewing limits, GpiSavePS / GpiRestorePS, GpiResetPS and
 * GpiSetAttrs / GpiQueryAttrs. Drawing is read back with GpiQueryPel (in
 * world coordinates, so under the identity transform for device pels).
 * Each check is shown in the window and written to stdout:
 *
 *   pm_xform_test: <name>: ok
 *   pm_xform_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_xform_test: done <passed>/<total>". The right half of the
 * window then shows a picture drawn through transforms: a rosette of
 * rotated squares, an ellipse made by scaling a circle, a sheared box,
 * a centimetre grid in PU_LOMETRIC and rotated lines of a star.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSFILEMGR
#include <os2.h>
#include <string.h>

#define N_CHECKS_MAX 40
#define LINE_H       16
#define COLUMN_ROWS  28    /* result lines in the first column */
#define PICTURE_X    340   /* left edge of the picture and second column */
#define PICTURE_H    226   /* height of the picture */

static HAB   hab;
static HWND  hwndClient;
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
    strcpy( line, "pm_xform_test: " );
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

static void identity( MATRIXLF *m )
{
    memset( m, 0, sizeof( *m ) );
    m->fxM11 = MAKEFIXED( 1, 0 );
    m->fxM22 = MAKEFIXED( 1, 0 );
    m->lM33 = 1;
}

static void set_model( HPS hps, MATRIXLF *m )
{
    GpiSetModelTransformMatrix( hps, 9, m, TRANSFORM_REPLACE );
}

static void reset_model( HPS hps )
{
    MATRIXLF m;
    identity( &m );
    set_model( hps, &m );
}

/* Convert one point between coordinate spaces; returns x * 10000 + y. */
static LONG convert( HPS hps, LONG from, LONG to, LONG x, LONG y )
{
    POINTL pt;
    pt.x = x;
    pt.y = y;
    if( !GpiConvert( hps, from, to, 1, &pt ) ) return -1;
    return pt.x * 10000 + pt.y;
}

/* ---------- the checks ---------- */

static void matrix_checks( HPS hps )
{
    MATRIXLF m;
    POINTL   pt;
    FIXED    afx[2];
    SIZEL    sizl;

    GpiErase( hps );
    GpiSetColor( hps, CLR_RED );

    memset( &m, 0, sizeof( m ) );
    GpiQueryModelTransformMatrix( hps, 9, &m );
    check( "model transform starts as identity",
           m.fxM11 == MAKEFIXED( 1, 0 ) && m.fxM22 == MAKEFIXED( 1, 0 ) && m.lM31 == 0 && m.lM33 == 1, TRUE );
    check( "GpiQueryPS page units are pels", GpiQueryPS( hps, &sizl ) & PS_UNITS, PU_PELS );

    /* Translation by (50, 20): world (0, 0) is device (50, 20). */
    pt.x = 50; pt.y = 20;
    check( "GpiTranslate", GpiTranslate( hps, &m, TRANSFORM_REPLACE, &pt ), TRUE );
    check( "GpiSetModelTransformMatrix", GpiSetModelTransformMatrix( hps, 9, &m, TRANSFORM_REPLACE ), TRUE );
    check( "GpiConvert world to device", convert( hps, CVTC_WORLD, CVTC_DEVICE, 0, 0 ), 500020 );
    check( "GpiConvert device to world", convert( hps, CVTC_DEVICE, CVTC_WORLD, 60, 25 ), 100005 );
    pt.x = 0; pt.y = 0;
    GpiSetPel( hps, &pt );
    check( "GpiQueryPel takes world points", pel_at( hps, 0, 0 ), CLR_RED );
    reset_model( hps );
    check( "the pel is at device (50,20)", pel_at( hps, 50, 20 ), CLR_RED );

    /* Scale x2 about the origin: the world box (10,10)-(19,19) has its
     * corners at device (20,20)-(38,38) (corners are points, not pels). */
    GpiErase( hps );
    afx[0] = afx[1] = MAKEFIXED( 2, 0 );
    pt.x = pt.y = 0;
    identity( &m );
    GpiScale( hps, &m, TRANSFORM_REPLACE, afx, &pt );
    set_model( hps, &m );
    box( hps, DRO_FILL, 10, 10, 19, 19 );
    reset_model( hps );
    check( "scaled box starts at (20,20)", pel_at( hps, 20, 20 ), CLR_RED );
    check( "scaled box ends at (38,38)", pel_at( hps, 38, 38 ), CLR_RED );
    check( "scaled box stops at (39,39)", pel_at( hps, 39, 39 ), CLR_BACKGROUND );

    /* Rotate 90 degrees about (100,50): the line (110,50)-(130,50) stands up. */
    GpiErase( hps );
    pt.x = 100; pt.y = 50;
    GpiRotate( hps, &m, TRANSFORM_REPLACE, MAKEFIXED( 90, 0 ), &pt );
    set_model( hps, &m );
    move_to( hps, 110, 50 );
    line_to( hps, 130, 50 );
    reset_model( hps );
    check( "rotated line is vertical", pel_at( hps, 100, 70 ), CLR_RED );
    check( "rotated line left (120,50)", pel_at( hps, 120, 50 ), CLR_BACKGROUND );

    /* ADD: translate after the scale. */
    afx[0] = afx[1] = MAKEFIXED( 3, 0 );
    pt.x = pt.y = 0;
    GpiScale( hps, &m, TRANSFORM_REPLACE, afx, &pt );
    pt.x = 7; pt.y = 0;
    GpiTranslate( hps, &m, TRANSFORM_ADD, &pt );
    check( "TRANSFORM_ADD scales first", m.lM31 == 7 && m.fxM11 == MAKEFIXED( 3, 0 ), TRUE );

    /* GpiConvertWithMatrix uses the given matrix only. */
    pt.x = 1; pt.y = 1;
    GpiConvertWithMatrix( hps, 1, &pt, 9, &m );
    check( "GpiConvertWithMatrix", pt.x * 100 + pt.y, 1003 );
}

static void state_checks( HPS hps )
{
    MATRIXLF   m;
    LINEBUNDLE lb;
    POINTL     pt;
    FIXED      afx[2];
    LONG       id;

    GpiSetColor( hps, CLR_RED );
    id = GpiSavePS( hps );
    check( "GpiSavePS returns level 1", id, 1 );
    GpiSetColor( hps, CLR_BLUE );
    afx[0] = afx[1] = MAKEFIXED( 3, 0 );
    pt.x = pt.y = 0;
    identity( &m );
    GpiScale( hps, &m, TRANSFORM_REPLACE, afx, &pt );
    set_model( hps, &m );
    check( "second GpiSavePS returns 2", GpiSavePS( hps ), 2 );
    GpiSetColor( hps, CLR_GREEN );
    check( "GpiRestorePS(-2)", GpiRestorePS( hps, -2 ), TRUE );
    check( "colour restored", GpiQueryColor( hps ), CLR_RED );
    GpiQueryModelTransformMatrix( hps, 9, &m );
    check( "model transform restored", m.fxM11, MAKEFIXED( 1, 0 ) );
    check( "the stack is empty again", GpiRestorePS( hps, -1 ), FALSE );

    /* GpiResetPS(GRES_ATTRS): model transform and current position reset. */
    set_model( hps, &m );
    move_to( hps, 33, 44 );
    GpiScale( hps, &m, TRANSFORM_REPLACE, afx, &pt );
    set_model( hps, &m );
    GpiResetPS( hps, GRES_ATTRS );
    GpiQueryModelTransformMatrix( hps, 9, &m );
    GpiQueryCurrentPosition( hps, &pt );
    check( "GpiResetPS resets transform and position",
           m.fxM11 == MAKEFIXED( 1, 0 ) && pt.x == 0 && pt.y == 0, TRUE );

    /* Attribute bundles. */
    memset( &lb, 0, sizeof( lb ) );
    lb.lColor = CLR_BLUE;
    lb.usType = LINETYPE_DOT;
    check( "GpiSetAttrs(PRIM_LINE)", GpiSetAttrs( hps, PRIM_LINE, LBB_COLOR | LBB_TYPE, 0, &lb ), TRUE );
    check( "GpiSetAttrs set the colour", GpiQueryColor( hps ), CLR_BLUE );
    check( "GpiSetAttrs set the line type", GpiQueryLineType( hps ), LINETYPE_DOT );
    memset( &lb, 0, sizeof( lb ) );
    check( "GpiQueryAttrs default mask",
           GpiQueryAttrs( hps, PRIM_LINE, LBB_COLOR | LBB_TYPE | LBB_WIDTH, &lb ), LBB_WIDTH );
    check( "GpiQueryAttrs line type", lb.usType, LINETYPE_DOT );
    GpiSetAttrs( hps, PRIM_LINE, LBB_COLOR | LBB_TYPE, LBB_COLOR | LBB_TYPE, &lb );
}

static void space_checks( HPS hps )
{
    MATRIXLF m;
    RECTL    rcl, old;
    POINTL   pt;
    FIXED    afx[2];
    SIZEL    sizl;
    HPS      hpsEng;
    HDC      hdc;

    /* Page viewport twice the page: page (5,5) is device (10,10). */
    GpiQueryPS( hps, &sizl );
    GpiQueryPageViewport( hps, &old );
    rcl.xLeft = rcl.yBottom = 0;
    rcl.xRight = 2 * sizl.cx;
    rcl.yTop = 2 * sizl.cy;
    check( "GpiSetPageViewport", GpiSetPageViewport( hps, &rcl ), TRUE );
    check( "page (5,5) is device (10,10)", convert( hps, CVTC_PAGE, CVTC_DEVICE, 5, 5 ), 100010 );
    GpiSetPageViewport( hps, &old );

    /* Clip rectangles are world coordinates. */
    GpiErase( hps );
    GpiSavePS( hps );
    afx[0] = afx[1] = MAKEFIXED( 2, 0 );
    pt.x = pt.y = 0;
    identity( &m );
    GpiScale( hps, &m, TRANSFORM_REPLACE, afx, &pt );
    set_model( hps, &m );
    rcl.xLeft = 5; rcl.yBottom = 5; rcl.xRight = 14; rcl.yTop = 9;
    GpiIntersectClipRectangle( hps, &rcl );
    memset( &rcl, 0, sizeof( rcl ) );
    GpiQueryClipBox( hps, &rcl );
    check( "GpiQueryClipBox in world coordinates",
           rcl.xLeft == 5 && rcl.yBottom == 5 && rcl.xRight == 14 && rcl.yTop == 9, TRUE );
    GpiSetColor( hps, CLR_RED );
    box( hps, DRO_FILL, 0, 0, 40, 40 );
    GpiRestorePS( hps, -1 );
    check( "clipped box inside (10,10)", pel_at( hps, 10, 10 ), CLR_RED );
    check( "clipped box inside (28,18)", pel_at( hps, 28, 18 ), CLR_RED );
    check( "clipped box outside (29,18)", pel_at( hps, 29, 18 ), CLR_BACKGROUND );
    GpiQueryClipBox( hps, &rcl );
    check( "GpiRestorePS drops the clip", rcl.xRight > 100, TRUE );

    /* Viewing limits clip drawing. */
    GpiErase( hps );
    rcl.xLeft = 0; rcl.yBottom = 0; rcl.xRight = 9; rcl.yTop = 9;
    GpiSetViewingLimits( hps, &rcl );
    GpiSetColor( hps, CLR_RED );
    box( hps, DRO_FILL, 0, 0, 30, 30 );
    check( "viewing limits keep (5,5)", pel_at( hps, 5, 5 ), CLR_RED );
    check( "viewing limits clip (15,15)", pel_at( hps, 15, 15 ), CLR_BACKGROUND );
    GpiResetPS( hps, GRES_ATTRS );

    /* A PU_LOENGLISH presentation space: 100 units = 1 inch = 96 pels. */
    hdc = WinOpenWindowDC( hwndClient );
    sizl.cx = sizl.cy = 0;
    hpsEng = GpiCreatePS( hab, hdc, &sizl, PU_LOENGLISH | GPIF_DEFAULT | GPIT_MICRO | GPIA_ASSOC );
    check( "GpiCreatePS(PU_LOENGLISH)", hpsEng != NULLHANDLE, TRUE );
    check( "GpiQueryPS reports PU_LOENGLISH", GpiQueryPS( hpsEng, &sizl ) & PS_UNITS, PU_LOENGLISH );
    check( "one inch is 96 pels", convert( hpsEng, CVTC_WORLD, CVTC_DEVICE, 100, 50 ), 960048 );
    GpiDestroyPS( hpsEng );
}

static void run_checks( HPS hps )
{
    matrix_checks( hps );
    state_checks( hps );
    space_checks( hps );
    GpiResetPS( hps, GRES_ALL );
    GpiErase( hps );
}

/* ---------- the picture (right half) ---------- */

static void draw_picture( HPS hps, PRECTL prcl )
{
    static const LONG colors[6] = { CLR_RED, CLR_DARKGREEN, CLR_BLUE, CLR_DARKPINK, CLR_DARKCYAN, CLR_BROWN };
    LONG     x0 = PICTURE_X, top = prcl->yTop, i;
    MATRIXLF m;
    POINTL   pt;
    FIXED    afx[2];
    SIZEL    sizl;
    HPS      hpsMetric;

    /* Rosette: a 40x40 square rotated in steps of 30 degrees. */
    pt.x = x0 + 60; pt.y = top - 60;
    for( i = 0; i < 12; i++ ) {
        identity( &m );
        GpiRotate( hps, &m, TRANSFORM_REPLACE, MAKEFIXED( 30 * i, 0 ), &pt );
        set_model( hps, &m );
        GpiSetColor( hps, colors[i % 6] );
        box( hps, DRO_OUTLINE, x0 + 60, top - 60, x0 + 100, top - 20 );
    }

    /* An ellipse: a circle of radius 20 scaled 2.5 x 1. */
    afx[0] = MAKEFIXED( 2, 0x8000 );
    afx[1] = MAKEFIXED( 1, 0 );
    pt.x = x0 + 200; pt.y = top - 60;
    identity( &m );
    GpiScale( hps, &m, TRANSFORM_REPLACE, afx, &pt );
    set_model( hps, &m );
    GpiSetColor( hps, CLR_DARKBLUE );
    move_to( hps, x0 + 200, top - 60 );
    GpiFullArc( hps, DRO_OUTLINEFILL, MAKEFIXED( 20, 0 ) );

    /* A sheared box. */
    identity( &m );
    m.fxM21 = MAKEFIXED( 0, 0x8000 );
    m.lM31 = x0 + 10;
    m.lM32 = top - 200;
    set_model( hps, &m );
    GpiSetColor( hps, CLR_DARKGREEN );
    box( hps, DRO_OUTLINEFILL, 0, 0, 60, 60 );

    /* A star of rotated lines. */
    reset_model( hps );
    pt.x = x0 + 150; pt.y = top - 165;
    GpiSetColor( hps, CLR_DARKRED );
    for( i = 0; i < 18; i++ ) {
        identity( &m );
        GpiRotate( hps, &m, TRANSFORM_REPLACE, MAKEFIXED( 20 * i, 0 ), &pt );
        set_model( hps, &m );
        move_to( hps, x0 + 150, top - 165 );
        line_to( hps, x0 + 190, top - 165 );
    }
    reset_model( hps );

    /* A centimetre grid in a PU_LOMETRIC presentation space (100 = 1 cm),
     * moved with the default view matrix. */
    sizl.cx = sizl.cy = 0;
    hpsMetric = GpiCreatePS( hab, WinOpenWindowDC( hwndClient ), &sizl,
                             PU_LOMETRIC | GPIF_DEFAULT | GPIT_MICRO | GPIA_ASSOC );
    if( hpsMetric != NULLHANDLE ) {
        /* The default view matrix works in page units: convert the device
         * point of the grid's corner first. */
        pt.x = x0 + 205; pt.y = top - 210;
        GpiConvert( hpsMetric, CVTC_DEVICE, CVTC_PAGE, 1, &pt );
        identity( &m );
        GpiTranslate( hpsMetric, &m, TRANSFORM_REPLACE, &pt );
        GpiSetDefaultViewMatrix( hpsMetric, 9, &m, TRANSFORM_REPLACE );
        GpiSetColor( hpsMetric, CLR_BLACK );
        for( i = 0; i <= 2; i++ ) {
            move_to( hpsMetric, i * 100, 0 );
            line_to( hpsMetric, i * 100, 200 );
            move_to( hpsMetric, 0, i * 100 );
            line_to( hpsMetric, 200, i * 100 );
        }
        GpiDestroyPS( hpsMetric );
    }
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
            strcpy( line, "pm_xform_test: done " );
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
    HWND  hwndFrame;
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

    WinRegisterClass( hab, "PMXformTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flFrameFlags,
                                    "PMXformTest", "Warpine GPI transforms test",
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
