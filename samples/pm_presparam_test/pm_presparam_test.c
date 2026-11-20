/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_presparam_test.c - presentation parameters, painting calls, static
 * control styles and class styles under Warpine
 *
 * The client (default frame, title bar and system menu) holds, in client
 * coordinates:
 *
 *   static text  SS_TEXT | DT_CENTER   ( 10, 400, 200, 24)  created with a
 *                PRESPARAMS block: red foreground, "12.Courier"
 *   SS_FGNDRECT                        (220, 400,  40, 24)  blue foreground
 *   SS_HALFTONERECT                    (270, 400,  40, 24)  black on white
 *   SS_FGNDFRAME                       (320, 400,  40, 24)  green foreground
 *   SS_SYSICON | SS_AUTOSIZE "#11"     (370, 440,   0,  0)  information icon
 *   SS_BITMAP "#10"                    (420, 440,   0,  0)  16 x 16 green bitmap
 *   push button                        ( 10, 340,  80, 30)  yellow background
 *   entry field                        (100, 340, 120, 24)  cyan background
 *   SS_GROUPBOX "Group"                (240, 300, 200, 80)  magenta border
 *   windows of three classes of its own: no class style, CS_SIZEREDRAW,
 *   CS_SYNCPAINT | CS_CLIPCHILDREN
 *
 * and a modeless dialog whose template gives the dialog a font and its
 * label a foreground colour (PRESPARAMS in the .rc).
 *
 * The client paints nothing itself, so what WM_ERASEBACKGROUND does shows.
 * A timer runs the checks once the window has painted; each is written to
 * stdout:
 *
 *   pm_presparam_test: <name>: ok
 *   pm_presparam_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_presparam_test: done <passed>/<total>". The window stays
 * until it is closed.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINERRORS
#include <os2.h>
#include <string.h>
#include "pm_presparam_test.h"

#define PREFIX    "pm_presparam_test: "
#define TID_STEP  1
#define STEP_MS   300

static HAB  hab;
static HWND frame, client;
static HWND sText, sFgRect, sHalftone, sFgFrame, sSysIcon, sBitmap, btn, entry, group;
static HWND plainKid, redrawKid, syncKid, dlg;
static int  n_checks = 0;
static int  n_passed = 0;
static int  step = 0;

/* What the client and the kids were sent. */
static int   n_erase = 0, erase_true = 0, erase_hps_ok = 0, erase_rect_ok = 0;
static int   n_ppchanged = 0;
static ULONG last_ppchanged = 0;
static int   sync_painted = 0;

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
    p = append_str( line, PREFIX );
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

/* ---------- pels ---------- */

/* The RGB value of client pel (x, y). */
static LONG pel( LONG x, LONG y )
{
    HPS    hps = WinGetPS( client );
    POINTL ptl;
    LONG   c;
    GpiCreateLogColorTable( hps, 0, LCOLF_RGB, 0, 0, NULL );
    ptl.x = x; ptl.y = y;
    c = GpiQueryPel( hps, &ptl );
    WinReleasePS( hps );
    return c;
}

/* Whether any pel of the client rectangle has colour rgb. */
static BOOL any_pel( LONG x1, LONG y1, LONG x2, LONG y2, LONG rgb )
{
    HPS    hps = WinGetPS( client );
    POINTL ptl;
    BOOL   found = FALSE;
    GpiCreateLogColorTable( hps, 0, LCOLF_RGB, 0, 0, NULL );
    for( ptl.y = y1; ptl.y < y2 && !found; ptl.y++ )
        for( ptl.x = x1; ptl.x < x2 && !found; ptl.x++ )
            found = GpiQueryPel( hps, &ptl ) == rgb;
    WinReleasePS( hps );
    return found;
}

static void fill( LONG x1, LONG y1, LONG x2, LONG y2, LONG clr )
{
    HPS   hps = WinGetPS( client );
    RECTL r;
    r.xLeft = x1; r.yBottom = y1; r.xRight = x2; r.yTop = y2;
    WinFillRect( hps, &r, clr );
    WinReleasePS( hps );
}

static BOOL set_rgb( HWND hwnd, ULONG id, ULONG rgb )
{
    return WinSetPresParam( hwnd, id, sizeof( rgb ), &rgb );
}

/* ---------- windows ---------- */

static MRESULT EXPENTRY KidProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_PAINT ) {
        RECTL r;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &r );
        WinEndPaint( hps );
        if( hwnd == syncKid ) sync_painted++;
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static HWND control( PSZ cls, PSZ text, ULONG style, LONG x, LONG y, LONG cx, LONG cy, ULONG id, PVOID pp )
{
    return WinCreateWindow( client, cls, text, WS_VISIBLE | style, x, y, cx, cy,
                            client, HWND_TOP, id, NULL, pp );
}

/* A PRESPARAMS block with a foreground colour and a font. */
static struct {
    ULONG cb;
    ULONG id1, cb1, rgb;
    ULONG id2, cb2;
    char  font[12];
} text_pp;

static void create_controls( void )
{
    text_pp.cb  = sizeof( text_pp ) - sizeof( ULONG );
    text_pp.id1 = PP_FOREGROUNDCOLOR; text_pp.cb1 = sizeof( ULONG ); text_pp.rgb = 0x00FF0000;
    text_pp.id2 = PP_FONTNAMESIZE;    text_pp.cb2 = sizeof( "12.Courier" );
    strcpy( text_pp.font, "12.Courier" );
    sText     = control( WC_STATIC, "Red text", SS_TEXT | DT_CENTER | DT_VCENTER, 10, 400, 200, 24, 1, &text_pp );
    sFgRect   = control( WC_STATIC, "", SS_FGNDRECT, 220, 400, 40, 24, 2, NULL );
    sHalftone = control( WC_STATIC, "", SS_HALFTONERECT, 270, 400, 40, 24, 3, NULL );
    sFgFrame  = control( WC_STATIC, "", SS_FGNDFRAME, 320, 400, 40, 24, 4, NULL );
    sSysIcon  = control( WC_STATIC, "#11", SS_SYSICON | SS_AUTOSIZE, 370, 440, 0, 0, 5, NULL );
    sBitmap   = control( WC_STATIC, "#10", SS_BITMAP, 420, 440, 0, 0, 6, NULL );
    btn       = control( WC_BUTTON, "Button", BS_PUSHBUTTON, 10, 340, 80, 30, 7, NULL );
    entry     = control( WC_ENTRYFIELD, "Entry", 0, 100, 340, 120, 24, 8, NULL );
    group     = control( WC_STATIC, "Group", SS_GROUPBOX, 240, 300, 200, 80, 9, NULL );
    set_rgb( sFgRect, PP_FOREGROUNDCOLOR, 0x000000FF );
    set_rgb( sHalftone, PP_FOREGROUNDCOLOR, 0x00000000 );
    set_rgb( sHalftone, PP_BACKGROUNDCOLOR, 0x00FFFFFF );
    set_rgb( sFgFrame, PP_FOREGROUNDCOLOR, 0x0000FF00 );
    set_rgb( btn, PP_BACKGROUNDCOLOR, 0x00FFFF00 );
    set_rgb( entry, PP_BACKGROUNDCOLOR, 0x0000FFFF );
    set_rgb( group, PP_BORDERCOLOR, 0x00FF00FF );
    plainKid  = control( "PlainKid", "", 0, 450, 200, 50, 50, 10, NULL );
    redrawKid = control( "RedrawKid", "", 0, 520, 200, 50, 50, 11, NULL );
    syncKid   = control( "SyncKid", "", 0, 590, 200, 40, 40, 12, NULL );
}

/* ---------- the checks ---------- */

static void check_presparams( void )
{
    char  buf[32];
    ULONG id, v, n;

    /* From the PRESPARAMS block of WinCreateWindow. */
    n = WinQueryPresParam( sText, PP_FONTNAMESIZE, 0, &id, sizeof( buf ), buf, 0 );
    check( "font from WinCreateWindow PRESPARAMS: length", n, sizeof( "12.Courier" ) );
    check( "font from WinCreateWindow PRESPARAMS: id", id, PP_FONTNAMESIZE );
    check( "font from WinCreateWindow PRESPARAMS: value", strcmp( buf, "12.Courier" ), 0 );
    n = WinQueryPresParam( sText, PP_FOREGROUNDCOLOR, PP_FOREGROUNDCOLORINDEX, &id, sizeof( v ), &v,
                           QPF_ID2COLORINDEX );
    check( "foreground colour query", n == 4 && v == 0x00FF0000, TRUE );

    /* A colour index turned into RGB. */
    v = CLR_RED;
    check( "WinSetPresParam index", WinSetPresParam( btn, PP_FOREGROUNDCOLORINDEX, sizeof( v ), &v ), TRUE );
    v = 0;
    n = WinQueryPresParam( btn, PP_FOREGROUNDCOLOR, PP_FOREGROUNDCOLORINDEX, &id, sizeof( v ), &v,
                           QPF_ID2COLORINDEX );
    check( "QPF_ID2COLORINDEX gives RGB", v, 0x00FF0000 );
    check( "QPF_ID2COLORINDEX reports the index id", id, PP_FOREGROUNDCOLORINDEX );
    n = WinQueryPresParam( btn, PP_FOREGROUNDCOLORINDEX, 0, NULL, sizeof( v ), &v, 0 );
    check( "without the flag the index", v, CLR_RED );

    /* Inherited from the owner unless QPF_NOINHERIT. */
    check( "WinSetPresParam on the client", WinSetPresParam( client, PP_USER, 4, "abc" ), TRUE );
    buf[0] = 0;
    n = WinQueryPresParam( sText, PP_USER, 0, NULL, sizeof( buf ), buf, 0 );
    check( "inherited from the owner", n == 4 && strcmp( buf, "abc" ) == 0, TRUE );
    check( "QPF_NOINHERIT", WinQueryPresParam( sText, PP_USER, 0, NULL, sizeof( buf ), buf, QPF_NOINHERIT ), 0 );
    check( "a short buffer gets part", WinQueryPresParam( client, PP_USER, 0, NULL, 2, buf, 0 ), 2 );

    /* Removal and errors. */
    check( "WinRemovePresParam", WinRemovePresParam( btn, PP_FOREGROUNDCOLORINDEX ), TRUE );
    check( "WinRemovePresParam again", WinRemovePresParam( btn, PP_FOREGROUNDCOLORINDEX ), FALSE );
    check( "removed: not found", WinQueryPresParam( btn, PP_FOREGROUNDCOLORINDEX, 0, NULL, 4, &v, 0 ), 0 );
    check( "invalid flag", WinQueryPresParam( btn, PP_USER, 0, NULL, 4, &v, 0x100 ), 0 );
    check( "invalid flag: error", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INVALID_FLAG );
    check( "bad window", WinSetPresParam( (HWND)0x7777, PP_USER, 4, "abc" ), FALSE );
}

static void check_controls( void )
{
    SWP  swp;
    LONG a, b;

    check( "static text in its foreground colour", any_pel( 10, 400, 210, 424, 0x00FF0000 ), TRUE );
    check( "SS_FGNDRECT", pel( 240, 410 ), 0x000000FF );
    a = pel( 290, 410 ); b = pel( 291, 410 );
    check( "SS_HALFTONERECT alternates", ( a ^ b ) == 0x00FFFFFF && ( a == 0 || b == 0 ), TRUE );
    check( "SS_FGNDFRAME edge", pel( 320, 410 ), 0x0000FF00 );
    check( "SS_FGNDFRAME inside", pel( 340, 410 ) != 0x0000FF00, TRUE );
    WinQueryWindowPos( sSysIcon, &swp );
    check( "SS_SYSICON | SS_AUTOSIZE size", swp.cx == 32 && swp.cy == 32, TRUE );
    check( "SS_SYSICON drawn", pel( swp.x + 16, swp.y + 4 ) != pel( swp.x + 40, swp.y + 4 ), TRUE );
    WinQueryWindowPos( sBitmap, &swp );
    check( "SS_BITMAP takes the bitmap's size", swp.cx == 16 && swp.cy == 16, TRUE );
    check( "SS_BITMAP drawn", pel( swp.x + 8, swp.y + 8 ), 0x0000C000 );
    check( "push button background", pel( 14, 344 ), 0x00FFFF00 );
    check( "entry field background", pel( 102, 342 ), 0x0000FFFF );
    check( "group box border colour", pel( 300, 300 ), 0x00FF00FF );
}

static void check_class_styles( void )
{
    RECTL r;
    ULONG style = WinQueryWindowULong( syncKid, QWL_STYLE );

    check( "CS_SYNCPAINT gives WS_SYNCPAINT", ( style & WS_SYNCPAINT ) != 0, TRUE );
    check( "CS_CLIPCHILDREN gives WS_CLIPCHILDREN", ( style & WS_CLIPCHILDREN ) != 0, TRUE );

    WinValidateRect( plainKid, NULL, FALSE );
    WinSetWindowPos( plainKid, NULLHANDLE, 0, 0, 40, 50, SWP_SIZE );
    check( "no CS_SIZEREDRAW: shrinking uncovers nothing", WinQueryUpdateRect( plainKid, &r ), FALSE );
    WinSetWindowPos( plainKid, NULLHANDLE, 0, 0, 60, 50, SWP_SIZE );
    WinQueryUpdateRect( plainKid, &r );
    check( "no CS_SIZEREDRAW: growing invalidates the new part",
           r.xLeft == 40 && r.yBottom == 0 && r.xRight == 60 && r.yTop == 50, TRUE );

    WinValidateRect( redrawKid, NULL, FALSE );
    WinSetWindowPos( redrawKid, NULLHANDLE, 0, 0, 40, 50, SWP_SIZE );
    check( "CS_SIZEREDRAW: shrinking invalidates the window", WinQueryUpdateRect( redrawKid, &r ), TRUE );
    check( "CS_SIZEREDRAW: all of it", r.xLeft == 0 && r.xRight == 40 && r.yTop == 50, TRUE );

    sync_painted = 0;
    WinInvalidateRect( syncKid, NULL, FALSE );
    check( "CS_SYNCPAINT paints before WinInvalidateRect returns", sync_painted, 1 );
}

static void check_painting( void )
{
    LONG window = WinQuerySysColor( HWND_DESKTOP, SYSCLR_WINDOW, 0 );
    int  before;

    check( "WM_ERASEBACKGROUND sent when the frame first paints", n_erase >= 1, TRUE );

    /* WinLockWindowUpdate: drawing into the locked window is dropped. */
    fill( 560, 100, 580, 120, CLR_GREEN );
    check( "drawn before the lock", pel( 570, 110 ), 0x0000FF00 );
    check( "WinLockWindowUpdate", WinLockWindowUpdate( HWND_DESKTOP, client ), TRUE );
    fill( 560, 100, 580, 120, CLR_RED );
    check( "nothing drawn while locked", pel( 570, 110 ), 0x0000FF00 );
    check( "one window locked at a time", WinLockWindowUpdate( HWND_DESKTOP, plainKid ), FALSE );
    check( "unlock", WinLockWindowUpdate( HWND_DESKTOP, NULLHANDLE ), TRUE );
    fill( 560, 100, 580, 120, CLR_RED );
    check( "drawn after the unlock", pel( 570, 110 ), 0x00FF0000 );

    /* WM_ERASEBACKGROUND: TRUE has the frame erase with SYSCLR_WINDOW. */
    fill( 500, 100, 520, 120, CLR_BLUE );
    check( "drawn before the erase", pel( 510, 110 ), 0x000000FF );
    before = n_erase;
    erase_true = 1;
    WinInvalidateRect( frame, NULL, FALSE );
    erase_true = 0;
    check( "WM_ERASEBACKGROUND sent when the frame is invalidated", n_erase, before + 1 );
    check( "WM_ERASEBACKGROUND mp1 is a presentation space", erase_hps_ok, TRUE );
    check( "WM_ERASEBACKGROUND mp2 is the rectangle", erase_rect_ok, TRUE );
    check( "TRUE: erased with SYSCLR_WINDOW", pel( 510, 110 ), window );
    before = n_erase;
    WinInvalidateRect( client, NULL, FALSE );
    check( "the client alone: no WM_ERASEBACKGROUND", n_erase, before );
}

static void check_dialog( void )
{
    char  buf[32];
    ULONG v = 0, id;
    HWND  label;

    dlg = WinLoadDlg( HWND_DESKTOP, client, WinDefDlgProc, NULLHANDLE, ID_DLG, NULL );
    check( "WinLoadDlg", dlg != NULLHANDLE, TRUE );
    label = WinWindowFromID( dlg, ID_DTEXT );
    WinQueryPresParam( label, PP_FOREGROUNDCOLOR, 0, &id, sizeof( v ), &v, QPF_NOINHERIT );
    check( "template PRESPARAMS of an item", v, 0x00FF0000 );
    buf[0] = 0;
    WinQueryPresParam( label, PP_FONTNAMESIZE, 0, NULL, sizeof( buf ), buf, 0 );
    check( "template PRESPARAMS of the dialog, inherited", strcmp( buf, "8.Helv" ), 0 );
}

static void run_step( void )
{
    char line[64], *p;
    switch( step++ ) {
    case 0:
        check_presparams();
        check_controls();
        check_class_styles();
        check_painting();
        check_dialog();
        break;
    case 1:
        check( "WM_PRESPARAMCHANGED posted", n_ppchanged >= 1, TRUE );
        check( "WM_PRESPARAMCHANGED mp1", last_ppchanged, PP_USER );
        WinStopTimer( hab, client, TID_STEP );
        p = append_str( line, PREFIX "done " );
        p = append_long( p, n_passed );
        p = append_str( p, "/" );
        p = append_long( p, n_checks );
        append_str( p, "\r\n" );
        write_stdout( line );
        break;
    }
}

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_ERASEBACKGROUND: {
        PRECTL r = (PRECTL)PVOIDFROMMP( mp2 );
        n_erase++;
        erase_hps_ok = LONGFROMMP( mp1 ) != 0;
        erase_rect_ok = r != NULL && r->xRight > r->xLeft && r->yTop > r->yBottom;
        return (MRESULT)erase_true;
    }
    case WM_PRESPARAMCHANGED:
        n_ppchanged++;
        last_ppchanged = LONGFROMMP( mp1 );
        return 0;
    case WM_TIMER:
        if( SHORT1FROMMP( mp1 ) == TID_STEP ) {
            run_step();
            return 0;
        }
        break;
    case WM_PAINT: {
        RECTL r;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &r );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG fl = FCF_TITLEBAR | FCF_SYSMENU;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "PresParamTest", ClientWndProc, 0, 0 );
    WinRegisterClass( hab, "PlainKid", KidProc, 0, 0 );
    WinRegisterClass( hab, "RedrawKid", KidProc, CS_SIZEREDRAW, 0 );
    WinRegisterClass( hab, "SyncKid", KidProc, CS_SYNCPAINT | CS_CLIPCHILDREN, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &fl, "PresParamTest",
                                "Presentation parameters", WS_VISIBLE, NULLHANDLE, 0, &client );
    create_controls();
    WinStartTimer( hab, client, TID_STEP, STEP_MS );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    if( dlg ) WinDestroyWindow( dlg );
    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
