/*
 * pm_demo.c - OS/2 PM Window API demo for Warpine
 *
 * Exercises: WinRegisterClass, WinCreateStdWindow, message loop,
 *   WinBeginPaint/WinEndPaint, GpiSetColor, GpiMove, GpiBox, GpiLine,
 *   GpiCharStringAt, GpiErase, WinFillRect, WinQueryWindowRect,
 *   WinQuerySysValue, WinSetWindowText, WinStartTimer/WinStopTimer,
 *   WinInvalidateRect.
 */
#define INCL_WIN
#define INCL_GPI
#include <os2.h>
#include <string.h>

#define ID_TIMER    1
#define TIMER_MS    1000

static int tick_count = 0;

/* Convert integer to string (avoids pulling in sprintf) */
static void int_to_str( int n, char *buf )
{
    char tmp[16];
    int  i = 0, neg = 0;

    if( n < 0 ) { neg = 1; n = -n; }
    if( n == 0 ) { tmp[i++] = '0'; }
    while( n > 0 ) {
        tmp[i++] = '0' + (n % 10);
        n /= 10;
    }
    if( neg ) tmp[i++] = '-';

    /* reverse */
    {
        int j = 0;
        while( i > 0 ) buf[j++] = tmp[--i];
        buf[j] = '\0';
    }
}

static void draw_scene( HWND hwnd, HPS hps )
{
    RECTL  rcl;
    POINTL pt;
    char   buf[64];

    WinQueryWindowRect( hwnd, &rcl );

    /* Clear background */
    WinFillRect( hps, &rcl, CLR_WHITE );

    /* Draw title text */
    GpiSetColor( hps, CLR_BLACK );
    pt.x = 20;
    pt.y = rcl.yTop - 30;
    GpiCharStringAt( hps, &pt, 28, "Warpine PM Demo - Phase 3" );

    /* Draw a filled red box */
    GpiSetColor( hps, CLR_RED );
    pt.x = 30;
    pt.y = rcl.yTop - 80;
    GpiMove( hps, &pt );
    pt.x = 180;
    pt.y = rcl.yTop - 180;
    GpiBox( hps, DRO_FILL, &pt, 0, 0 );

    /* Draw a blue outline box */
    GpiSetColor( hps, CLR_BLUE );
    pt.x = 200;
    pt.y = rcl.yTop - 80;
    GpiMove( hps, &pt );
    pt.x = 350;
    pt.y = rcl.yTop - 180;
    GpiBox( hps, DRO_OUTLINE, &pt, 0, 0 );

    /* Draw green diagonal lines */
    GpiSetColor( hps, CLR_GREEN );
    pt.x = 380;
    pt.y = rcl.yTop - 80;
    GpiMove( hps, &pt );
    pt.x = 530;
    pt.y = rcl.yTop - 180;
    GpiLine( hps, &pt );

    pt.x = 530;
    pt.y = rcl.yTop - 80;
    GpiMove( hps, &pt );
    pt.x = 380;
    pt.y = rcl.yTop - 180;
    GpiLine( hps, &pt );

    /* Draw cyan label for the X */
    GpiSetColor( hps, CLR_CYAN );
    pt.x = 420;
    pt.y = rcl.yTop - 200;
    GpiCharStringAt( hps, &pt, 13, "Diagonal Lines" );

    /* Show system info */
    {
        LONG cx = WinQuerySysValue( HWND_DESKTOP, SV_CXSCREEN );
        LONG cy = WinQuerySysValue( HWND_DESKTOP, SV_CYSCREEN );
        char line[64];

        GpiSetColor( hps, CLR_BLACK );

        strcpy( line, "Screen: " );
        int_to_str( cx, buf );
        strcat( line, buf );
        strcat( line, "x" );
        int_to_str( cy, buf );
        strcat( line, buf );

        pt.x = 20;
        pt.y = rcl.yTop - 220;
        GpiCharStringAt( hps, &pt, strlen( line ), line );
    }

    /* Show tick counter */
    {
        char line[64];
        strcpy( line, "Timer ticks: " );
        int_to_str( tick_count, buf );
        strcat( line, buf );

        pt.x = 20;
        pt.y = rcl.yTop - 250;
        GpiCharStringAt( hps, &pt, strlen( line ), line );
    }

    /* Draw a pink filled-outline box */
    GpiSetColor( hps, CLR_PINK );
    pt.x = 30;
    pt.y = rcl.yTop - 290;
    GpiMove( hps, &pt );
    pt.x = 560;
    pt.y = rcl.yTop - 340;
    GpiBox( hps, DRO_OUTLINEFILL, &pt, 0, 0 );

    /* Label inside the pink box */
    GpiSetColor( hps, CLR_BLACK );
    pt.x = 40;
    pt.y = rcl.yTop - 320;
    GpiCharStringAt( hps, &pt, 37, "DRO_OUTLINEFILL box with text inside" );

    /* Yellow filled box at bottom */
    GpiSetColor( hps, CLR_YELLOW );
    pt.x = 30;
    pt.y = 20;
    GpiMove( hps, &pt );
    pt.x = 200;
    pt.y = 80;
    GpiBox( hps, DRO_FILL, &pt, 0, 0 );

    GpiSetColor( hps, CLR_BLACK );
    pt.x = 45;
    pt.y = 45;
    GpiCharStringAt( hps, &pt, 16, "Bottom-left test" );
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PAINT:
    {
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        draw_scene( hwnd, hps );
        WinEndPaint( hps );
        return 0;
    }
    case WM_TIMER:
    {
        tick_count++;
        /* Update title bar with tick count */
        {
            char title[64];
            char buf[16];
            strcpy( title, "PM Demo - Tick " );
            int_to_str( tick_count, buf );
            strcat( title, buf );
            WinSetWindowText( WinQueryWindow( hwnd, QW_PARENT ), title );
        }
        WinInvalidateRect( hwnd, NULL, FALSE );
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
    HAB    hab;
    HMQ    hmq;
    HWND   hwndFrame, hwndClient;
    QMSG   qmsg;
    ULONG  flFrameFlags = FCF_TITLEBAR | FCF_SIZEBORDER | FCF_MINMAX |
                          FCF_SYSMENU | FCF_TASKLIST;

    hab = WinInitialize( 0 );
    if( hab == 0 ) return 1;

    hmq = WinCreateMsgQueue( hab, 0 );
    if( hmq == 0 ) {
        WinTerminate( hab );
        return 1;
    }

    WinRegisterClass( hab, "PMDemo", ClientWndProc, CS_SIZEREDRAW, 0 );

    hwndFrame = WinCreateStdWindow(
        HWND_DESKTOP, WS_VISIBLE,
        &flFrameFlags, "PMDemo",
        "Warpine PM Demo",
        0, NULLHANDLE, 0, &hwndClient );

    if( hwndFrame == 0 ) {
        WinDestroyMsgQueue( hmq );
        WinTerminate( hab );
        return 1;
    }

    /* Start a 1-second timer */
    WinStartTimer( hab, hwndClient, ID_TIMER, TIMER_MS );

    /* Message loop */
    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) {
        WinDispatchMsg( hab, &qmsg );
    }

    WinStopTimer( hab, hwndClient, ID_TIMER );
    WinDestroyWindow( hwndFrame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );

    return 0;
}
