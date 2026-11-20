/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_bitmap_test.c - memory presentation spaces, bitmaps and bit-block
 * transfers for Warpine
 *
 * The window shows a scene drawn through the APIs under test:
 *   - a 16x16 bitmap drawn through a memory presentation space (blue with
 *     one red pel), copied to the window with GpiBitBlt, once 1:1 and once
 *     stretched to 32x32;
 *   - an 8x8 RT_BITMAP resource (GpiLoadBitmap) drawn with WinDrawBitmap;
 *   - the bitmap with the same id from BMPRES.DLL (DosLoadModule +
 *     GpiLoadBitmap with its module handle), stored RLE8-compressed;
 *   - a dotted line (GpiSetLineType).
 * On the first WM_PAINT the scene is checked with GpiQueryPel and the other
 * query functions. Each check is shown in the window and written to stdout:
 *
 *   pm_bitmap_test: <name>: ok
 *   pm_bitmap_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_bitmap_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_GPIBITMAPS
#define INCL_GPILOGCOLORTABLE
#define INCL_DEV
#define INCL_DOSFILEMGR
#define INCL_DOSMODULEMGR
#include <os2.h>
#include <string.h>

#define ID_QUADRANTS 1
#define N_CHECKS_MAX 32
#define LINE_H       15
#define MEM_CX       16
#define MEM_CY       16
#define BMP_CX       8
/* The scene sits on the right, clear of the result lines on the left. */
#define BLIT_X       420    /* 1:1 copy of the memory bitmap */
#define BLIT_Y       10
#define STRETCH_X    450    /* the same, stretched to 32x32 */
#define STRETCH_Y    10
#define DRAWBMP_X    500    /* WinDrawBitmap of the resource */
#define DRAWBMP_Y    10
#define DLLBMP_X     515    /* WinDrawBitmap of the DLL's resource */
#define DLLBMP_Y     10
#define LINE_X       420    /* dotted line, 16 pels long */
#define LINE_Y       60
#define RGB_PEL_X    530    /* the pel drawn in RGB mode */
#define RGB_PEL_Y    60

static HAB     hab;
static HDC     hdcMem;
static HPS     hpsMem;
static HBITMAP hbmMem, hbmRes, hbmDll;
static HMODULE hmodDll;
static APIRET  rcDll;
static BOOL    checked = FALSE;
static int     n_checks = 0;
static int     n_passed = 0;
static char    results[N_CHECKS_MAX][80];
static BYTE    bmi_buf[sizeof( BITMAPINFOHEADER2 ) + 256 * sizeof( RGB2 )];
static BYTE    bits[MEM_CY * MEM_CX * 3];

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
    strcpy( line, "pm_bitmap_test: " );
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

static BOOL is_white( LONG pel )
{
    return pel == CLR_BACKGROUND || pel == CLR_WHITE;
}

static void box( HPS hps, LONG x1, LONG y1, LONG x2, LONG y2 )
{
    POINTL pt;
    pt.x = x1; pt.y = y1;
    GpiMove( hps, &pt );
    pt.x = x2; pt.y = y2;
    GpiBox( hps, DRO_FILL, &pt, 0, 0 );
}

static void line( HPS hps, LONG x1, LONG y1, LONG x2, LONG y2 )
{
    POINTL pt;
    pt.x = x1; pt.y = y1;
    GpiMove( hps, &pt );
    pt.x = x2; pt.y = y2;
    GpiLine( hps, &pt );
}

/* A BITMAPINFOHEADER2 of the given depth, all other fields zero. */
static PBITMAPINFOHEADER2 header2( ULONG cx, ULONG cy, USHORT bits_per_pel )
{
    static BITMAPINFOHEADER2 bmih;
    memset( &bmih, 0, sizeof( bmih ) );
    bmih.cbFix     = sizeof( bmih );
    bmih.cx        = cx;
    bmih.cy        = cy;
    bmih.cPlanes   = 1;
    bmih.cBitCount = bits_per_pel;
    return &bmih;
}

/* ---------- resources and scene ---------- */

static void create_resources( HPS hpsWin )
{
    SIZEL sizl = { 0, 0 };

    hdcMem = DevOpenDC( hab, OD_MEMORY, "*", 0L, NULL, NULLHANDLE );
    hpsMem = GpiCreatePS( hab, hdcMem, &sizl, PU_PELS | GPIT_MICRO | GPIA_ASSOC );
    hbmMem = GpiCreateBitmap( hpsMem, header2( MEM_CX, MEM_CY, 24 ), 0, NULL, NULL );
    GpiSetBitmap( hpsMem, hbmMem );
    GpiSetColor( hpsMem, CLR_BLUE );
    box( hpsMem, 0, 0, MEM_CX - 1, MEM_CY - 1 );
    GpiSetColor( hpsMem, CLR_RED );
    {
        POINTL pt = { 3, 3 };
        GpiSetPel( hpsMem, &pt );
    }
    hbmRes = GpiLoadBitmap( hpsWin, NULLHANDLE, ID_QUADRANTS, 0, 0 );
    {
        UCHAR failed[64];
        rcDll = DosLoadModule( failed, sizeof( failed ), "BMPRES", &hmodDll );
        if( rcDll == 0 )
            hbmDll = GpiLoadBitmap( hpsWin, hmodDll, ID_QUADRANTS, 0, 0 );
    }
}

static void draw_scene( HPS hps )
{
    POINTL aptl[4];

    /* 1:1: target (x, y)-(x+16, y+16), exclusive; source lower-left. */
    aptl[0].x = BLIT_X;          aptl[0].y = BLIT_Y;
    aptl[1].x = BLIT_X + MEM_CX; aptl[1].y = BLIT_Y + MEM_CY;
    aptl[2].x = 0;               aptl[2].y = 0;
    GpiBitBlt( hps, hpsMem, 3, aptl, ROP_SRCCOPY, BBO_IGNORE );

    /* Stretched x2: four points. */
    aptl[0].x = STRETCH_X;              aptl[0].y = STRETCH_Y;
    aptl[1].x = STRETCH_X + 2 * MEM_CX; aptl[1].y = STRETCH_Y + 2 * MEM_CY;
    aptl[2].x = 0;                      aptl[2].y = 0;
    aptl[3].x = MEM_CX;                 aptl[3].y = MEM_CY;
    GpiBitBlt( hps, hpsMem, 4, aptl, ROP_SRCCOPY, BBO_IGNORE );

    if( hbmRes ) {
        POINTL pt = { DRAWBMP_X, DRAWBMP_Y };
        WinDrawBitmap( hps, hbmRes, NULL, &pt, 0, 0, DBM_NORMAL );
    }
    if( hbmDll ) {
        POINTL pt = { DLLBMP_X, DLLBMP_Y };
        WinDrawBitmap( hps, hbmDll, NULL, &pt, 0, 0, DBM_NORMAL );
    }

    GpiSetColor( hps, CLR_RED );
    GpiSetLineType( hps, LINETYPE_DOT );
    line( hps, LINE_X, LINE_Y, LINE_X + 15, LINE_Y );
    GpiSetLineType( hps, LINETYPE_DEFAULT );
}

/* ---------- the checks ---------- */

static void run_checks( HPS hps )
{
    PBITMAPINFO2      pbmi = (PBITMAPINFO2)bmi_buf;
    BITMAPINFOHEADER2 info;
    POINTL            aptl[2];
    POINTL            pt;
    LONG              before, n, i, caps;
    LONG              table[1];
    HBITMAP           hbmTmp;

    check( "memory PS draws into its bitmap", pel_at( hpsMem, 5, 5 ), CLR_BLUE );
    check( "GpiSetPel in the bitmap", pel_at( hpsMem, 3, 3 ), CLR_RED );
    check( "outside the bitmap", pel_at( hpsMem, MEM_CX, 0 ), CLR_NOINDEX );

    check( "GpiBitBlt copies to the window", pel_at( hps, BLIT_X + 5, BLIT_Y + 5 ), CLR_BLUE );
    check( "GpiBitBlt copies the red pel", pel_at( hps, BLIT_X + 3, BLIT_Y + 3 ), CLR_RED );
    check( "GpiBitBlt target is exclusive",
           is_white( pel_at( hps, BLIT_X + MEM_CX, BLIT_Y + 5 ) ), TRUE );
    check( "stretched GpiBitBlt doubles pels",
           pel_at( hps, STRETCH_X + 7, STRETCH_Y + 7 ), CLR_RED );
    check( "stretched GpiBitBlt fills 32x32",
           pel_at( hps, STRETCH_X + 31, STRETCH_Y + 31 ), CLR_BLUE );

    memset( bmi_buf, 0, sizeof( bmi_buf ) );
    pbmi->cbFix = 16;
    pbmi->cPlanes = 1;
    pbmi->cBitCount = 24;
    n = GpiQueryBitmapBits( hpsMem, 0, MEM_CY, (PBYTE)bits, pbmi );
    check( "GpiQueryBitmapBits returns every row", n, MEM_CY );
    check( "GpiQueryBitmapBits fills in cx", (LONG)pbmi->cx, MEM_CX );
    check( "row 0 starts blue (B, G, R)", bits[0] == 0xFF && bits[1] == 0 && bits[2] == 0, TRUE );

    check( "GpiLoadBitmap", hbmRes != NULLHANDLE, TRUE );
    memset( &info, 0, sizeof( info ) );
    info.cbFix = sizeof( info );
    GpiQueryBitmapInfoHeader( hbmRes, &info );
    check( "loaded bitmap is 8 pels wide", (LONG)info.cx, BMP_CX );
    check( "WinDrawBitmap: red quadrant", pel_at( hps, DRAWBMP_X + 1, DRAWBMP_Y + 1 ), CLR_RED );
    check( "WinDrawBitmap: green quadrant", pel_at( hps, DRAWBMP_X + 6, DRAWBMP_Y + 1 ), CLR_GREEN );
    check( "WinDrawBitmap: blue quadrant", pel_at( hps, DRAWBMP_X + 1, DRAWBMP_Y + 6 ), CLR_BLUE );

    check( "DosLoadModule BMPRES", (LONG)rcDll, 0 );
    check( "GpiLoadBitmap from the DLL", hbmDll != NULLHANDLE, TRUE );
    check( "DLL bitmap (RLE8): yellow left", pel_at( hps, DLLBMP_X + 1, DLLBMP_Y + 1 ), CLR_YELLOW );
    check( "DLL bitmap (RLE8): magenta right", pel_at( hps, DLLBMP_X + 6, DLLBMP_Y + 6 ), CLR_PINK );

    n = 0;
    for( i = LINE_X; i <= LINE_X + 15; i++ )
        if( pel_at( hps, i, LINE_Y ) == CLR_RED ) n++;
    check( "dotted line skips pels", n > 0 && n < 16, TRUE );

    /* ROP_DSTINVERT needs no source; twice restores. */
    aptl[0].x = BLIT_X;     aptl[0].y = BLIT_Y;
    aptl[1].x = BLIT_X + 4; aptl[1].y = BLIT_Y + 4;
    before = pel_at( hps, BLIT_X + 1, BLIT_Y + 1 );
    GpiBitBlt( hps, NULLHANDLE, 2, aptl, ROP_DSTINVERT, BBO_IGNORE );
    check( "ROP_DSTINVERT changes the pel", pel_at( hps, BLIT_X + 1, BLIT_Y + 1 ) != before, TRUE );
    GpiBitBlt( hps, NULLHANDLE, 2, aptl, ROP_DSTINVERT, BBO_IGNORE );
    check( "ROP_DSTINVERT twice restores", pel_at( hps, BLIT_X + 1, BLIT_Y + 1 ), before );

    /* RGB colour values after GpiCreateLogColorTable(LCOLF_RGB). */
    GpiCreateLogColorTable( hps, 0, LCOLF_RGB, 0, 0, NULL );
    GpiSetColor( hps, 0x00FF8000 );
    pt.x = RGB_PEL_X; pt.y = RGB_PEL_Y;
    GpiSetPel( hps, &pt );
    check( "LCOLF_RGB: GpiQueryPel answers in RGB", pel_at( hps, RGB_PEL_X, RGB_PEL_Y ), 0x00FF8000 );
    GpiCreateLogColorTable( hps, LCOL_RESET, LCOLF_DEFAULT, 0, 0, NULL );
    table[0] = 0x00123456;
    GpiCreateLogColorTable( hps, 0, LCOLF_CONSECRGB, 16, 1, table );
    check( "LCOLF_CONSECRGB: GpiQueryRGBColor", GpiQueryRGBColor( hps, 0, 16 ), 0x00123456 );
    GpiCreateLogColorTable( hps, LCOL_RESET, LCOLF_DEFAULT, 0, 0, NULL );

    /* A bitmap cannot be deleted while selected. */
    hbmTmp = GpiCreateBitmap( hpsMem, header2( 2, 2, 24 ), 0, NULL, NULL );
    check( "GpiSetBitmap returns the previous bitmap", GpiSetBitmap( hpsMem, hbmTmp ) == hbmMem, TRUE );
    check( "GpiDeleteBitmap refuses a selected bitmap", GpiDeleteBitmap( hbmTmp ), FALSE );
    GpiSetBitmap( hpsMem, hbmMem );
    check( "GpiDeleteBitmap after deselecting", GpiDeleteBitmap( hbmTmp ), TRUE );

    DevQueryCaps( hdcMem, CAPS_COLOR_BITCOUNT, 1, &caps );
    check( "DevQueryCaps reports a colour depth", caps > 0, TRUE );
}

static void draw_results( HPS hps, PRECTL prcl )
{
    POINTL pt;
    char   text[80];
    int    i;

    GpiSetColor( hps, CLR_BLACK );
    pt.x = 8;
    pt.y = prcl->yTop - LINE_H;
    for( i = 0; i < n_checks; i++ ) {
        GpiCharStringAt( hps, &pt, strlen( results[i] ), results[i] );
        pt.y -= LINE_H;
    }
    strcpy( text, "passed " );
    append_long( text + strlen( text ), n_passed );
    strcat( text, "/" );
    append_long( text + strlen( text ), n_checks );
    GpiCharStringAt( hps, &pt, strlen( text ), text );
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PAINT:
    {
        RECTL rcl;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        WinQueryWindowRect( hwnd, &rcl );
        WinFillRect( hps, &rcl, CLR_WHITE );
        if( !hdcMem ) create_resources( hps );
        draw_scene( hps );
        if( !checked ) {
            char text[64];
            checked = TRUE;
            run_checks( hps );
            strcpy( text, "pm_bitmap_test: done " );
            append_long( text + strlen( text ), n_passed );
            strcat( text, "/" );
            append_long( text + strlen( text ), n_checks );
            strcat( text, "\r\n" );
            write_stdout( text );
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

    WinRegisterClass( hab, "PMBitmapTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flFrameFlags,
                                    "PMBitmapTest", "Warpine bitmap and BitBlt test",
                                    0, NULLHANDLE, 0, &hwndClient );
    if( hwndFrame == 0 ) {
        WinDestroyMsgQueue( hmq );
        WinTerminate( hab );
        return 1;
    }

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    if( hbmRes ) GpiDeleteBitmap( hbmRes );
    if( hbmDll ) GpiDeleteBitmap( hbmDll );
    if( hmodDll ) DosFreeModule( hmodDll );
    GpiSetBitmap( hpsMem, NULLHANDLE );
    GpiDeleteBitmap( hbmMem );
    GpiDestroyPS( hpsMem );
    DevCloseDC( hdcMem );
    WinDestroyWindow( hwndFrame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
