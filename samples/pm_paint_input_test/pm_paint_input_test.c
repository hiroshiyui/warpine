/*
 * pm_paint_input_test.c - update regions, child presentation spaces,
 * pointer routing, WM_HITTEST, WinQueryQueueStatus and must-complete
 * sections, under Warpine (ADR-0058)
 *
 * "Paint and input" is a 400 x 300 frame at (40, 200) whose client holds:
 *
 *   Kid    (20, 20) 100 x 60, the program's own class, with a static over
 *          its left half, a grandchild Grand at (70, 10) 20 x 20 and a
 *          grandchild Wide at (90, 40) 30 x 30 that sticks out of it;
 *   Under  (200, 20) 100 x 60, and over it Glass, of a CS_HITTEST class
 *          answering HT_TRANSPARENT;
 *   Shut   (320, 20) 60 x 60, of a CS_HITTEST class answering HT_DISCARD.
 *
 * At start it checks, without input:
 *
 *   - update regions: two invalidated rectangles stay two (the gap between
 *     them is not in WinQueryUpdateRegion), a validated rectangle leaves a
 *     hole, and the WinBeginPaint presentation space paints only the region;
 *   - a child window's presentation space stays inside its parent;
 *   - WinQueryQueueStatus: a posted message is in the high word, and in the
 *     low word only the first time;
 *   - must-complete sections: a signal sent to itself waits until
 *     DosExitMustComplete, which fails with ERROR_ALREADY_RESET outside one.
 *
 * The GUI script then clicks (desktop coordinates, top-left origin):
 * Kid's static (Kid gets it), Grand, Glass (Under gets it), Shut (nobody),
 * the client, and Kid with button 2. Each check is written to stdout:
 *
 *   pm_paint_input_test: <name>: ok
 *   pm_paint_input_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_paint_input_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOS
#define INCL_DOSERRORS
#define INCL_DOSEXCEPTIONS
#include <os2.h>
#include <string.h>
#include <stdio.h>

#define ID_POLL    1
#define POLL_MS    100
#define WAIT_POLLS 150            /* 15 s for the script's clicks */
#define CLICKS     6
#define WM_PROBE   (WM_USER + 5)

static HAB  hab;
static HWND frame, client, kid, grand, wide, under, glass, shut;
static int  n_checks = 0, n_passed = 0;
static int  step = 0, waited = 0;
static volatile LONG signal_count = 0;

/* The button messages the windows got: who, which, where. */
static struct { HWND hwnd; ULONG msg; SHORT x, y; } got[16];
static int  n_got = 0;
static MPARAM hit_mp1 = 0, hit_mp2 = (MPARAM)-1;
static int  hit_asked = 0;

static void write_stdout( const char *s )
{
    ULONG written;
    DosWrite( 1, (PVOID)s, strlen( s ), &written );
}

static void check( const char *what, LONG gotv, LONG want )
{
    char line[200];
    if( gotv == want ) {
        sprintf( line, "pm_paint_input_test: %s: ok\r\n", what );
        n_passed++;
    } else {
        sprintf( line, "pm_paint_input_test: %s: FAIL got %ld want %ld\r\n", what, gotv, want );
    }
    write_stdout( line );
    n_checks++;
}

static void record( HWND hwnd, ULONG msg, MPARAM mp1 )
{
    if( n_got < 16 && ( msg == WM_BUTTON1DOWN || msg == WM_BUTTON2DOWN ) ) {
        got[n_got].hwnd = hwnd;
        got[n_got].msg = msg;
        got[n_got].x = SHORT1FROMMP( mp1 );
        got[n_got].y = SHORT2FROMMP( mp1 );
        n_got++;
    }
}

static ULONG APIENTRY signal_handler( PEXCEPTIONREPORTRECORD rep, PEXCEPTIONREGISTRATIONRECORD reg,
                                      PCONTEXTRECORD ctx, PVOID unused )
{
    (void)reg; (void)ctx; (void)unused;
    if( rep->ExceptionNum == XCPT_SIGNAL ) {
        signal_count++;
        return XCPT_CONTINUE_EXECUTION;
    }
    return XCPT_CONTINUE_SEARCH;
}

static ULONG own_pid( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    return ppib->pib_ulpid;
}

/* The colour-table index of the client's pel at (x, y). */
static LONG pel( LONG x, LONG y )
{
    HPS    hps = WinGetPS( client );
    POINTL p;
    LONG   v;
    p.x = x;
    p.y = y;
    v = GpiQueryPel( hps, &p );
    WinReleasePS( hps );
    return v;
}

static void fill( HWND hwnd, LONG color )
{
    RECTL rc;
    HPS   hps = WinGetPS( hwnd );
    WinQueryWindowRect( hwnd, &rc );
    WinFillRect( hps, &rc, color );
    WinReleasePS( hps );
}

static LONG inside( HPS hps, HRGN hrgn, LONG x, LONG y )
{
    POINTL p;
    p.x = x;
    p.y = y;
    return GpiPtInRegion( hps, hrgn, &p );
}

static void paint_checks( void )
{
    RECTL rc, box;
    HPS   hps, paint;
    HRGN  hrgn;

    fill( client, CLR_WHITE );
    WinValidateRect( client, NULL, TRUE );
    /* Two rectangles, and a hole in the first. */
    rc.xLeft = 10;  rc.yBottom = 100; rc.xRight = 70;  rc.yTop = 160;
    WinInvalidateRect( client, &rc, FALSE );
    rc.xLeft = 200; rc.yBottom = 200; rc.xRight = 230; rc.yTop = 230;
    WinInvalidateRect( client, &rc, FALSE );
    rc.xLeft = 30;  rc.yBottom = 120; rc.xRight = 50;  rc.yTop = 140;
    WinValidateRect( client, &rc, FALSE );
    hps = WinGetPS( client );
    hrgn = GpiCreateRegion( hps, 0, NULL );
    check( "WinQueryUpdateRegion is complex", WinQueryUpdateRegion( client, hrgn ), RGN_COMPLEX );
    GpiQueryRegionBox( hps, hrgn, &box );
    check( "  its box", box.xLeft == 10 && box.yBottom == 100 && box.xRight == 230 && box.yTop == 230, TRUE );
    check( "  a point of the first part", inside( hps, hrgn, 15, 105 ), PRGN_INSIDE );
    check( "  the hole is not in it", inside( hps, hrgn, 40, 130 ), PRGN_OUTSIDE );
    check( "  nor the gap between the parts", inside( hps, hrgn, 150, 180 ), PRGN_OUTSIDE );
    check( "  a point of the second part", inside( hps, hrgn, 215, 215 ), PRGN_INSIDE );
    GpiDestroyRegion( hps, hrgn );
    WinReleasePS( hps );
    /* WinBeginPaint paints exactly the update region. */
    paint = WinBeginPaint( client, NULLHANDLE, &rc );
    check( "WinBeginPaint's rectangle bounds the region",
           rc.xLeft == 10 && rc.yBottom == 100 && rc.xRight == 230 && rc.yTop == 230, TRUE );
    WinQueryWindowRect( client, &rc );
    WinFillRect( paint, &rc, CLR_RED );
    WinEndPaint( paint );
    check( "  painted in the region", pel( 15, 105 ), CLR_RED );
    check( "  not in the hole", pel( 40, 130 ) != CLR_RED, TRUE );
    check( "  nor between the parts", pel( 150, 180 ) != CLR_RED, TRUE );
    check( "  and in the second part", pel( 215, 215 ), CLR_RED );
    check( "  the window is valid then", WinQueryUpdateRect( client, NULL ), FALSE );
    /* Wide spans client (110..140, 60..90); Kid (20..120, 20..80). */
    fill( client, CLR_WHITE );
    fill( wide, CLR_RED );
    check( "a grandchild's PS paints inside its parent", pel( 115, 70 ), CLR_RED );
    check( "  not right of the parent", pel( 130, 70 ) != CLR_RED, TRUE );
    check( "  nor above it", pel( 115, 85 ) != CLR_RED, TRUE );
}

static void queue_checks( void )
{
    ULONG s;
    QMSG  q;

    WinQueryQueueStatus( HWND_DESKTOP );
    WinPostMsg( client, WM_PROBE, 0, 0 );
    s = WinQueryQueueStatus( HWND_DESKTOP );
    check( "WinQueryQueueStatus: a posted message waits (high word)", ( s >> 16 & QS_POSTMSG ) != 0, TRUE );
    check( "  and came since the last call (low word)", ( s & QS_POSTMSG ) != 0, TRUE );
    s = WinQueryQueueStatus( HWND_DESKTOP );
    check( "  the next call: still waiting", ( s >> 16 & QS_POSTMSG ) != 0, TRUE );
    check( "  but not new", ( s & QS_POSTMSG ) != 0, FALSE );
    WinPeekMsg( hab, &q, client, WM_PROBE, WM_PROBE, PM_REMOVE );
}

static void must_complete_checks( void )
{
    ULONG nest = 99;

    check( "DosExitMustComplete outside a section", DosExitMustComplete( &nest ), ERROR_ALREADY_RESET );
    check( "DosEnterMustComplete", DosEnterMustComplete( &nest ), NO_ERROR );
    check( "  nesting 1", nest, 1 );
    DosEnterMustComplete( &nest );
    check( "  nesting 2", nest, 2 );
    DosSendSignalException( own_pid(), XCPT_SIGNAL_INTR );
    DosSleep( 200 );
    check( "a signal waits in the section", signal_count, 0 );
    DosExitMustComplete( &nest );
    DosSleep( 50 );
    check( "  and in the outer one", signal_count, 0 );
    DosExitMustComplete( &nest );
    check( "  nesting 0", nest, 0 );
    DosSleep( 50 );
    check( "it comes once the section ends", signal_count, 1 );
}

/* The windows that got the script's clicks, in order. */
static void click_checks( void )
{
    check( "six button messages", n_got, CLICKS );
    if( n_got < CLICKS ) return;
    check( "a click on a child's static goes to the child", got[0].hwnd == kid, TRUE );
    check( "  in its coordinates", got[0].x == 10 && got[0].y == 30, TRUE );
    check( "a click on a grandchild goes to it", got[1].hwnd == grand, TRUE );
    check( "  in its coordinates", got[1].x == 5 && got[1].y == 5, TRUE );
    check( "WM_HITTEST asked of the CS_HITTEST window", hit_asked > 0, TRUE );
    check( "  mp1 the point in its coordinates", SHORT1FROMMP( hit_mp1 ) == 50 && SHORT2FROMMP( hit_mp1 ) == 30, TRUE );
    check( "  mp2 0", (LONG)hit_mp2, 0 );
    check( "HT_TRANSPARENT: the window beneath gets the click", got[2].hwnd == under, TRUE );
    check( "HT_DISCARD: nobody gets it; the client gets the next", got[3].hwnd == client, TRUE );
    check( "  at its point", got[3].x == 150 && got[3].y == 200, TRUE );
    check( "button 2 goes to the child too", got[4].hwnd == kid && got[4].msg == WM_BUTTON2DOWN, TRUE );
    check( "a click on a disabled grandchild goes to its parent", got[5].hwnd == kid, TRUE );
}

static void poll( void )
{
    waited++;
    switch( step ) {
    case 0:
        paint_checks();
        queue_checks();
        must_complete_checks();
        step = 1;
        waited = 0;
        return;
    case 1:
        /* The script's fifth click; then Grand is disabled for the last. */
        if( n_got >= 5 && WinIsWindowEnabled( grand ) ) WinEnableWindow( grand, FALSE );
        if( n_got < CLICKS && waited <= WAIT_POLLS ) return;
        click_checks();
        WinStopTimer( hab, client, ID_POLL );
        WinPostMsg( client, WM_QUIT, 0, 0 );
        return;
    }
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    record( hwnd, msg, mp1 );
    switch( msg ) {
    case WM_TIMER:
        if( SHORT1FROMMP( mp1 ) == ID_POLL ) { poll(); return 0; }
        break;
    case WM_PAINT: {
        RECTL rc;
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

MRESULT EXPENTRY KidWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    record( hwnd, msg, mp1 );
    if( msg == WM_PAINT ) {
        RECTL rc;
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinEndPaint( hps );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

MRESULT EXPENTRY GlassWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    record( hwnd, msg, mp1 );
    if( msg == WM_HITTEST ) {
        hit_asked++;
        hit_mp1 = mp1;
        hit_mp2 = mp2;
        return (MRESULT)HT_TRANSPARENT;
    }
    return KidWndProc( hwnd, msg, mp1, mp2 );
}

MRESULT EXPENTRY ShutWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    record( hwnd, msg, mp1 );
    if( msg == WM_HITTEST ) return (MRESULT)HT_DISCARD;
    return KidWndProc( hwnd, msg, mp1, mp2 );
}

static HWND child( PSZ cls, HWND parent, LONG x, LONG y, LONG cx, LONG cy )
{
    return WinCreateWindow( parent, cls, NULL, WS_VISIBLE, x, y, cx, cy, parent, HWND_TOP, 0, NULL, NULL );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU;
    ULONG times;
    char  line[64];
    EXCEPTIONREGISTRATIONRECORD rec;

    rec.prev_structure = NULL;
    rec.ExceptionHandler = signal_handler;
    DosSetExceptionHandler( &rec );
    DosSetSignalExceptionFocus( SIG_SETFOCUS, &times );

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "PITest", ClientWndProc, CS_SIZEREDRAW, 0 );
    WinRegisterClass( hab, "PIKid", KidWndProc, 0, 0 );
    WinRegisterClass( hab, "PIGlass", GlassWndProc, CS_HITTEST, 0 );
    WinRegisterClass( hab, "PIShut", ShutWndProc, CS_HITTEST, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "PITest", "Paint and input", 0, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 40, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    kid   = child( "PIKid", client, 20, 20, 100, 60 );
    WinCreateWindow( kid, WC_STATIC, "label", WS_VISIBLE | SS_TEXT, 0, 0, 50, 60, kid, HWND_TOP, 0, NULL, NULL );
    grand = child( "PIKid", kid, 70, 10, 20, 20 );
    wide  = child( "PIKid", kid, 90, 40, 30, 30 );
    under = child( "PIKid", client, 200, 20, 100, 60 );
    glass = child( "PIGlass", client, 200, 20, 100, 60 );
    shut  = child( "PIShut", client, 320, 20, 60, 60 );
    WinStartTimer( hab, client, ID_POLL, POLL_MS );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    sprintf( line, "pm_paint_input_test: done %d/%d\r\n", n_passed, n_checks );
    write_stdout( line );
    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    DosUnsetExceptionHandler( &rec );
    return 0;
}
