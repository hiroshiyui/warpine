/*
 * pm_textpos_test.c - GPI text layout checks for Warpine
 *
 * On its first WM_PAINT the window exercises the GPI text-layout calls and
 * character attributes: GpiQueryCharStringPos(At) with and without an
 * increment vector, GpiQueryWidthTable, GpiQueryTextBox (TXTBOX_* order,
 * character angle, alignment, shear), GpiSetCharExtra / GpiSetCharDirection
 * / GpiSetTextAlignment / GpiSetCharShear / GpiSetCharAngle and their
 * queries, GpiCharStringPosAt with CHS_OPAQUE and CHS_CLIP read back with
 * GpiQueryPel, and rotated text read back with GpiQueryPel. Each check is
 * written to stdout:
 *
 *   pm_textpos_test: <name>: ok
 *   pm_textpos_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_textpos_test: done <passed>/<total>". Every paint then
 * shows the results and a picture of text at several angles, sheared,
 * top to bottom, right to left, spaced by a vector, centred, underscored
 * and struck out.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSFILEMGR
#include <os2.h>
#include <string.h>

#define N_CHECKS_MAX 40
#define LINE_H       12
#define PREFIX       "pm_textpos_test: "

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

static LONG pel_at( HPS hps, LONG x, LONG y )
{
    POINTL pt;
    pt.x = x;
    pt.y = y;
    return GpiQueryPel( hps, &pt );
}

/* Pels of colour `clr` in the inclusive rectangle (x1, y1) - (x2, y2). */
static LONG count_pels( HPS hps, LONG x1, LONG y1, LONG x2, LONG y2, LONG clr )
{
    LONG x, y, n = 0;
    for( y = y1; y <= y2; y++ )
        for( x = x1; x <= x2; x++ )
            if( pel_at( hps, x, y ) == clr ) n++;
    return n;
}

static void set_angle( HPS hps, LONG x, LONG y )
{
    GRADIENTL g;
    g.x = x;
    g.y = y;
    GpiSetCharAngle( hps, &g );
}

static void set_shear( HPS hps, LONG x, LONG y )
{
    POINTL pt;
    pt.x = x;
    pt.y = y;
    GpiSetCharShear( hps, &pt );
}

static void set_char_height( HPS hps, LONG cy )
{
    SIZEF box;
    box.cx = MAKEFIXED( cy, 0 );
    box.cy = MAKEFIXED( cy, 0 );
    GpiSetCharBox( hps, &box );
}

static void reset_text( HPS hps )
{
    set_angle( hps, 1, 0 );
    set_shear( hps, 0, 1 );
    GpiSetCharDirection( hps, CHDIRN_LEFTRIGHT );
    GpiSetCharExtra( hps, 0 );
    GpiSetCharBreakExtra( hps, 0 );
    GpiSetTextAlignment( hps, TA_NORMAL_HORIZ, TA_NORMAL_VERT );
    set_char_height( hps, 0 );
}

/* ---------- the checks ---------- */

static void run_checks( HPS hps )
{
    POINTL    start, pos[4], box[TXTBOX_COUNT];
    LONG      adx[2], widths[2], lh, lv;
    FIXED     fx;
    GRADIENTL grad;
    RECTL     rcl;

    GpiErase( hps );
    reset_text( hps );

    /* Positions with an increment vector. */
    start.x = 40; start.y = 300;
    adx[0] = 20; adx[1] = 30;
    check( "QueryCharStringPosAt vector",
           GpiQueryCharStringPosAt( hps, &start, CHS_VECTOR, 2, "AB", adx, pos ), TRUE );
    check( "vector pos 1", pos[1].x - pos[0].x, 20 );
    check( "vector concat", pos[2].x - pos[0].x, 50 );
    check( "vector baseline", pos[2].y, 300 );

    /* Widths agree with the positions. */
    check( "QueryWidthTable", GpiQueryWidthTable( hps, 'A', 2, widths ), TRUE );
    check( "width of A > 0", widths[0] > 0, TRUE );
    GpiQueryCharStringPosAt( hps, &start, 0, 2, "AB", NULL, pos );
    check( "pos 1 = width of A", pos[1].x - pos[0].x, widths[0] );

    /* The text box: TXTBOX_* order and the same concatenation point. */
    check( "QueryTextBox", GpiQueryTextBox( hps, 2, "AB", TXTBOX_COUNT, box ), TRUE );
    check( "box concat = position concat", box[TXTBOX_CONCAT].x, pos[2].x - pos[0].x );
    check( "TOPRIGHT above baseline", box[TXTBOX_TOPRIGHT].y > 0, TRUE );
    check( "BOTTOMRIGHT below baseline", box[TXTBOX_BOTTOMRIGHT].y < 0, TRUE );

    /* Character extra widens every character. */
    check( "SetCharExtra", GpiSetCharExtra( hps, MAKEFIXED( 4, 0 ) ), TRUE );
    GpiQueryCharExtra( hps, &fx );
    check( "QueryCharExtra", fx, MAKEFIXED( 4, 0 ) );
    GpiQueryCharStringPosAt( hps, &start, 0, 2, "AB", NULL, pos );
    check( "extra spacing", pos[1].x - pos[0].x, widths[0] + 4 );
    GpiSetCharExtra( hps, 0 );

    /* Right to left: the string ends left of the start. */
    check( "SetCharDirection RIGHTLEFT", GpiSetCharDirection( hps, CHDIRN_RIGHTLEFT ), TRUE );
    check( "QueryCharDirection", GpiQueryCharDirection( hps ), CHDIRN_RIGHTLEFT );
    GpiQueryCharStringPosAt( hps, &start, 0, 2, "AB", NULL, pos );
    check( "right to left first char", pos[0].x, 40 - widths[0] );
    check( "SetCharDirection rejects 5", GpiSetCharDirection( hps, 5 ), FALSE );
    GpiSetCharDirection( hps, CHDIRN_LEFTRIGHT );

    /* Centred alignment: the box straddles the start point. */
    check( "SetTextAlignment", GpiSetTextAlignment( hps, TA_CENTER, TA_BASE ), TRUE );
    GpiQueryTextAlignment( hps, &lh, &lv );
    check( "QueryTextAlignment", lh * 0x10000 + lv, TA_CENTER * 0x10000 + TA_BASE );
    GpiQueryTextBox( hps, 2, "AB", TXTBOX_COUNT, box );
    check( "centred box", box[TXTBOX_TOPLEFT].x + box[TXTBOX_TOPRIGHT].x >= -1 &&
                          box[TXTBOX_TOPLEFT].x + box[TXTBOX_TOPRIGHT].x <= 1, TRUE );
    check( "SetTextAlignment rejects 9", GpiSetTextAlignment( hps, 9, TA_BASE ), FALSE );
    GpiSetTextAlignment( hps, TA_NORMAL_HORIZ, TA_NORMAL_VERT );

    /* A quarter turn: the string runs up. */
    set_angle( hps, 0, 1 );
    GpiQueryCharAngle( hps, &grad );
    check( "QueryCharAngle", grad.x * 100 + grad.y, 1 );
    GpiQueryTextBox( hps, 2, "AB", TXTBOX_COUNT, box );
    check( "rotated concat x", box[TXTBOX_CONCAT].x, 0 );
    check( "rotated concat y", box[TXTBOX_CONCAT].y, widths[0] + widths[1] );
    set_angle( hps, 1, 0 );

    /* Shear: the top of the box leans right. */
    pos[3].x = 1; pos[3].y = 0;
    check( "SetCharShear rejects y = 0", GpiSetCharShear( hps, &pos[3] ), FALSE );
    set_shear( hps, 1, 1 );
    GpiQueryTextBox( hps, 2, "AB", TXTBOX_COUNT, box );
    check( "sheared top left", box[TXTBOX_TOPLEFT].x, box[TXTBOX_TOPLEFT].y );
    set_shear( hps, 0, 1 );

    /* CHS_OPAQUE fills the rectangle; CHS_CLIP keeps the text inside. */
    GpiSetColor( hps, CLR_RED );
    GpiSetBackColor( hps, CLR_YELLOW );
    set_char_height( hps, 32 );   /* stems wide enough for fully covered pels */
    rcl.xLeft = 300; rcl.yBottom = 200; rcl.xRight = 330; rcl.yTop = 230;
    start.x = 305; start.y = 210;
    check( "CharStringPosAt", GpiCharStringPosAt( hps, &start, &rcl, CHS_OPAQUE | CHS_CLIP,
                                                 6, "HHHHHH", NULL ), GPI_OK );
    check( "opaque corner", pel_at( hps, 300, 200 ), CLR_YELLOW );
    check( "opaque top right", pel_at( hps, 329, 229 ), CLR_YELLOW );
    check( "outside opaque", pel_at( hps, 330, 210 ), CLR_BACKGROUND );
    check( "red inside", count_pels( hps, 300, 200, 329, 229, CLR_RED ) > 0, TRUE );
    check( "clipped right of rect", count_pels( hps, 330, 200, 380, 229, CLR_RED ), 0 );

    /* Rotated text: a column above and left of the start point. */
    GpiSetColor( hps, CLR_BLUE );
    set_angle( hps, 0, 1 );
    start.x = 450; start.y = 150;
    GpiCharStringAt( hps, &start, 3, "III" );
    check( "rotated text drawn", count_pels( hps, 418, 150, 450, 200, CLR_BLUE ) > 0, TRUE );
    check( "nothing right of it", count_pels( hps, 452, 150, 480, 200, CLR_BLUE ), 0 );
    GpiQueryCurrentPosition( hps, &pos[0] );
    check( "position moved up", pos[0].x == 450 && pos[0].y > 150, TRUE );
    reset_text( hps );
}

/* ---------- the picture ---------- */

static void text_at( HPS hps, LONG x, LONG y, const char *s )
{
    POINTL pt;
    pt.x = x;
    pt.y = y;
    GpiCharStringAt( hps, &pt, strlen( s ), (PCH)s );
}

static void mark( HPS hps, LONG x, LONG y )
{
    POINTL pt;
    GpiSetColor( hps, CLR_RED );
    pt.x = x - 4; pt.y = y; GpiMove( hps, &pt );
    pt.x = x + 4; GpiLine( hps, &pt );
    pt.x = x; pt.y = y - 4; GpiMove( hps, &pt );
    pt.y = y + 4; GpiLine( hps, &pt );
}

static void draw_picture( HPS hps, LONG left, LONG top )
{
    POINTL pt;
    LONG   adx[6] = { 30, 30, 30, 30, 30, 30 };
    LONG   cx = left + 150, cy = top - 150;

    reset_text( hps );
    set_char_height( hps, 20 );

    /* A fan of angles around one point. */
    GpiSetColor( hps, CLR_DARKBLUE );
    set_angle( hps, 1, 0 );   text_at( hps, cx, cy, "   0 deg" );
    set_angle( hps, 1, 1 );   text_at( hps, cx, cy, "   45 deg" );
    set_angle( hps, 0, 1 );   text_at( hps, cx, cy, "   90 deg" );
    set_angle( hps, -1, 1 );  text_at( hps, cx, cy, "   135 deg" );
    set_angle( hps, -1, 0 );  text_at( hps, cx, cy, "   180 deg" );
    set_angle( hps, 0, -1 );  text_at( hps, cx, cy, "   270 deg" );
    set_angle( hps, 1, 0 );
    mark( hps, cx, cy );

    /* Sheared. */
    GpiSetColor( hps, CLR_DARKGREEN );
    set_shear( hps, 1, 2 );
    text_at( hps, left + 10, top - 330, "Sheared" );
    set_shear( hps, 0, 1 );

    /* Top to bottom. */
    GpiSetColor( hps, CLR_DARKPINK );
    GpiSetCharDirection( hps, CHDIRN_TOPBOTTOM );
    text_at( hps, left + 300, top - 10, "DOWN" );
    GpiSetCharDirection( hps, CHDIRN_LEFTRIGHT );

    /* Right to left from a mark. */
    GpiSetColor( hps, CLR_BROWN );
    GpiSetCharDirection( hps, CHDIRN_RIGHTLEFT );
    text_at( hps, left + 290, top - 370, "LTR" );
    GpiSetCharDirection( hps, CHDIRN_LEFTRIGHT );
    mark( hps, left + 290, top - 370 );

    /* Spaced by a vector, underscored. */
    GpiSetColor( hps, CLR_BLACK );
    pt.x = left + 10; pt.y = top - 370;
    GpiCharStringPosAt( hps, &pt, NULL, CHS_VECTOR | CHS_UNDERSCORE, 6, "VECTOR", adx );

    /* Centred on a mark, struck out, on an opaque box. */
    GpiSetColor( hps, CLR_DARKCYAN );
    GpiSetTextAlignment( hps, TA_CENTER, TA_HALF );
    pt.x = left + 150; pt.y = top - 410;
    {
        RECTL rcl;
        GpiSetBackColor( hps, CLR_PALEGRAY );
        rcl.xLeft = left + 60; rcl.yBottom = top - 425;
        rcl.xRight = left + 240; rcl.yTop = top - 395;
        GpiCharStringPosAt( hps, &pt, &rcl, CHS_OPAQUE | CHS_STRIKEOUT, 7, "Centred", NULL );
    }
    GpiSetTextAlignment( hps, TA_NORMAL_HORIZ, TA_NORMAL_VERT );
    mark( hps, left + 150, top - 410 );
    reset_text( hps );
}

static void draw_results( HPS hps, PRECTL prcl )
{
    POINTL pt;
    char   line[80];
    int    i;
    FATTRS fat;

    WinFillRect( hps, prcl, CLR_WHITE );
    /* Results in a small font down the left side. */
    memset( &fat, 0, sizeof( fat ) );
    fat.usRecordLength = sizeof( fat );
    strcpy( fat.szFacename, "Helv" );
    fat.lMaxBaselineExt = 11;
    GpiCreateLogFont( hps, NULL, 1, &fat );
    GpiSetCharSet( hps, 1 );
    GpiSetColor( hps, CLR_BLACK );
    pt.x = 6;
    pt.y = prcl->yTop - LINE_H;
    for( i = 0; i < n_checks; i++ ) {
        GpiCharStringAt( hps, &pt, strlen( results[i] ), results[i] );
        pt.y -= LINE_H;
    }
    strcpy( line, "passed " );
    append_long( line + strlen( line ), n_passed );
    strcat( line, "/" );
    append_long( line + strlen( line ), n_checks );
    GpiCharStringAt( hps, &pt, strlen( line ), line );
    GpiSetCharSet( hps, LCID_DEFAULT );
    GpiDeleteSetId( hps, 1 );
    draw_picture( hps, 230, prcl->yTop - 10 );
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

    WinRegisterClass( hab, "PMTextPosTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, 0, &flFrameFlags,
                                    "PMTextPosTest", "Warpine GPI text layout test",
                                    0, NULLHANDLE, 0, &hwndClient );
    if( hwndFrame == 0 ) {
        WinDestroyMsgQueue( hmq );
        WinTerminate( hab );
        return 1;
    }
    WinSetWindowPos( hwndFrame, HWND_TOP, 20, 40, 720, 520,
                     SWP_SIZE | SWP_MOVE | SWP_SHOW | SWP_ACTIVATE );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( hwndFrame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
