/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_circslider_test.c - the circular slider (WC_CIRCULARSLIDER) under Warpine
 *
 * The client (default frame at desktop (32, 32), title bar only) holds:
 *
 *   dial 30  "Volume", the default styles, at ( 40, 160) 200 x 220
 *   dial 31  "Heading", CSS_360 | CSS_CIRCULARVALUE | CSS_POINTSELECT |
 *            CSS_NOBUTTON | CSS_MIDPOINT, at (300, 160) 200 x 220
 *
 * main() checks the CSM_* messages first (WinRegisterCircularSlider, the
 * default range and increments, refused ranges, increments and values,
 * CSM_QUERYRADIUS, CSM_SETBITMAPDATA with a red picture for Volume's -
 * button), sets dial 30 to 0..50 in steps of
 * 5 at 20 and dial 31 to 0..359, and gives dial 30 the focus. The
 * integration test then presses Right, Page Up and End on dial 30, clicks
 * its + button (at the end: nothing changes) and its - button, presses
 * on dial 31 right of its centre and drags below it, and closes the window
 * with Alt+F4. The notifications are logged and checked at the end. Each
 * check is written to stdout:
 *
 *   pm_circslider_test: <name>: ok
 *   pm_circslider_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_circslider_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINCIRCULARSLIDER
#define INCL_DOSPROCESS
#define INCL_DEV
#include <os2.h>
#include <string.h>

/* In os2386.lib (PMCTLS.24) but not in Open Watcom's headers. */
BOOL APIENTRY WinRegisterCircularSlider( VOID );

#define ID_VOLUME  30
#define ID_HEADING 31

static HAB  hab;
static HWND frame, client, volume, heading;
static int  n_checks = 0;
static int  n_passed = 0;

/* What the owner was told during the session. */
static int   n_changed[2], n_tracking[2], n_focus;
static LONG  last_changed[2] = { -1, -1 }, last_tracking[2] = { -1, -1 };

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
    p = append_str( line, "pm_circslider_test: " );
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

/* ---------- the client ---------- */

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_CONTROL: {
        int i = SHORT1FROMMP( mp1 ) == ID_VOLUME ? 0 : SHORT1FROMMP( mp1 ) == ID_HEADING ? 1 : -1;
        if( i < 0 ) return 0;
        switch( SHORT2FROMMP( mp1 ) ) {
        case CSN_CHANGED:  n_changed[i]++; last_changed[i] = (SHORT)SHORT1FROMMP( mp2 ); break;
        case CSN_TRACKING: n_tracking[i]++; last_tracking[i] = (SHORT)SHORT1FROMMP( mp2 ); break;
        case CSN_SETFOCUS: if( i == 0 && SHORT1FROMMP( mp2 ) ) n_focus++; break;
        }
        return 0;
    }
    case WM_PAINT: {
        RECTL rc;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinFillRect( hps, &rc, CLR_WHITE );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static HWND dial( PSZ text, ULONG style, LONG x, LONG y, ULONG id )
{
    return WinCreateWindow( client, WC_CIRCULARSLIDER, text, WS_VISIBLE | WS_TABSTOP | style,
                            x, y, 200, 220, client, HWND_TOP, id, NULL, NULL );
}

/* A red 8 x 8 bitmap: Volume's - button picture. */
static HBITMAP red_bitmap( void )
{
    static BYTE bits[8 * 24];
    BITMAPINFOHEADER2 bmih;
    BITMAPINFO2       info;
    SIZEL sizl = { 0, 0 };
    HDC   hdc = DevOpenDC( hab, OD_MEMORY, "*", 0L, NULL, NULLHANDLE );
    HPS   hps = GpiCreatePS( hab, hdc, &sizl, PU_PELS | GPIT_MICRO | GPIA_ASSOC );
    int   i;

    for( i = 0; i < (int)sizeof( bits ); i += 3 ) {
        bits[i] = 0; bits[i + 1] = 0; bits[i + 2] = 0xFF;      /* B, G, R */
    }
    memset( &bmih, 0, sizeof( bmih ) );
    bmih.cbFix = sizeof( bmih );
    bmih.cx = 8; bmih.cy = 8; bmih.cPlanes = 1; bmih.cBitCount = 24;
    memset( &info, 0, sizeof( info ) );
    info.cbFix = 16;
    info.cx = 8; info.cy = 8; info.cPlanes = 1; info.cBitCount = 24;
    return GpiCreateBitmap( hps, &bmih, CBM_INIT, bits, &info );
}

static void check_messages( void )
{
    SHORT        lo = -1, hi = -1, v = -1;
    USHORT       scroll = 0, tick = 0, radius = 0;
    CSBITMAPDATA bd;

    check( "the dials exist", volume != NULLHANDLE && heading != NULLHANDLE, TRUE );
    check( "CSM_QUERYRANGE", (LONG)WinSendMsg( volume, CSM_QUERYRANGE, MPFROMP( &lo ), MPFROMP( &hi ) ), TRUE );
    check( "  the default range starts at 0", lo, 0 );
    check( "  and ends at 100", hi, 100 );
    check( "CSM_QUERYINCREMENT", (LONG)WinSendMsg( volume, CSM_QUERYINCREMENT, MPFROMP( &scroll ), MPFROMP( &tick ) ), TRUE );
    check( "  a step of 1", scroll, 1 );
    check( "  a tick every 10", tick, 10 );
    check( "CSM_SETRANGE low above high", (LONG)WinSendMsg( volume, CSM_SETRANGE, MPFROMSHORT( 10 ), MPFROMSHORT( 5 ) ), FALSE );
    check( "CSM_SETRANGE 0..50", (LONG)WinSendMsg( volume, CSM_SETRANGE, MPFROMSHORT( 0 ), MPFROMSHORT( 50 ) ), TRUE );
    check( "CSM_SETINCREMENT with no step", (LONG)WinSendMsg( volume, CSM_SETINCREMENT, MPFROMSHORT( 0 ), MPFROMSHORT( 5 ) ), FALSE );
    check( "CSM_SETINCREMENT 5, 10", (LONG)WinSendMsg( volume, CSM_SETINCREMENT, MPFROMSHORT( 5 ), MPFROMSHORT( 10 ) ), TRUE );
    WinSendMsg( volume, CSM_QUERYINCREMENT, MPFROMP( &scroll ), MPFROMP( &tick ) );
    check( "  queried back", scroll * 100 + tick, 510 );
    check( "CSM_SETVALUE out of the range", (LONG)WinSendMsg( volume, CSM_SETVALUE, MPFROMSHORT( 99 ), 0 ), FALSE );
    check( "CSM_SETVALUE 20", (LONG)WinSendMsg( volume, CSM_SETVALUE, MPFROMSHORT( 20 ), 0 ), TRUE );
    WinSendMsg( volume, CSM_QUERYVALUE, MPFROMP( &v ), 0 );
    check( "CSM_QUERYVALUE", v, 20 );
    check( "CSM_QUERYRADIUS", (LONG)WinSendMsg( volume, CSM_QUERYRADIUS, MPFROMP( &radius ), 0 ), TRUE );
    check( "  a dial that fits", radius > 20 && radius <= 100, TRUE );
    bd.hbmLeftUp = red_bitmap();
    bd.hbmLeftDown = bd.hbmRightUp = bd.hbmRightDown = NULLHANDLE;
    check( "a bitmap for the - button", bd.hbmLeftUp != NULLHANDLE, TRUE );
    check( "CSM_SETBITMAPDATA", (LONG)WinSendMsg( volume, CSM_SETBITMAPDATA, MPFROMP( &bd ), 0 ), TRUE );
    check( "CSM_SETRANGE 0..359", (LONG)WinSendMsg( heading, CSM_SETRANGE, MPFROMSHORT( 0 ), MPFROMSHORT( 359 ) ), TRUE );
}

static void check_session( void )
{
    SHORT v = -1;

    check( "the focus was announced", n_focus >= 1, TRUE );
    WinSendMsg( volume, CSM_QUERYVALUE, MPFROMP( &v ), 0 );
    check( "Volume: Right, Page Up, End, +, - leave 45", v, 45 );
    check( "  four changes (+ at the end changed nothing)", n_changed[0], 4 );
    check( "  the last one 45", last_changed[0], 45 );
    check( "  no tracking", n_tracking[0], 0 );
    WinSendMsg( heading, CSM_QUERYVALUE, MPFROMP( &v ), 0 );
    check( "Heading: a press right of the centre, a drag below it: 180", v, 180 );
    check( "  tracked from 90 on the way to 180", n_tracking[1] >= 2, TRUE );
    check( "  the last tracked 180", last_tracking[1], 180 );
    check( "  one change on release", n_changed[1], 1 );
    check( "  of 180", last_changed[1], 180 );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_BORDER;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    check( "WinRegisterCircularSlider", WinRegisterCircularSlider(), TRUE );
    WinRegisterClass( hab, "CircSliderTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "CircSliderTest",
                                "PM Circular Slider Test", WS_VISIBLE, NULLHANDLE, 0, &client );
    volume  = dial( "Volume", 0, 40, 160, ID_VOLUME );
    heading = dial( "Heading", CSS_360 | CSS_CIRCULARVALUE | CSS_POINTSELECT | CSS_NOBUTTON | CSS_MIDPOINT,
                    300, 160, ID_HEADING );

    check_messages();
    /* Notifications of the checks above are still queued: drain them first. */
    while( WinPeekMsg( hab, &qmsg, NULLHANDLE, 0, 0, PM_REMOVE ) ) {
        if( qmsg.msg == WM_QUIT ) break;
        WinDispatchMsg( hab, &qmsg );
    }
    n_changed[0] = n_changed[1] = n_tracking[0] = n_tracking[1] = n_focus = 0;
    WinSetFocus( HWND_DESKTOP, volume );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    check_session();

    p = append_str( line, "pm_circslider_test: done " );
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
