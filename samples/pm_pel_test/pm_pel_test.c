/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_pel_test.c - GPI read-back and mix-mode checks for Warpine
 *
 * On its first WM_PAINT the window draws with GpiSetPel, GpiBox and GpiLine
 * under several mix modes and reads the result back with GpiQueryPel. Each
 * check is shown in the window and written to stdout:
 *
 *   pm_pel_test: <name>: ok
 *   pm_pel_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_pel_test: done <passed>/<total>".
 *
 * Only default colour-table indices are used, and the mix checks compare a
 * pel with itself before and after (XOR twice, invert twice), so the
 * expected results do not depend on the display's colour depth.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSFILEMGR
#include <os2.h>
#include <string.h>

#define N_CHECKS_MAX 16
#define LINE_H       18

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
    strcpy( line, "pm_pel_test: " );
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

static void box( HPS hps, LONG x1, LONG y1, LONG x2, LONG y2 )
{
    POINTL pt;
    pt.x = x1; pt.y = y1;
    GpiMove( hps, &pt );
    pt.x = x2; pt.y = y2;
    GpiBox( hps, DRO_FILL, &pt, 0, 0 );
}

/* ---------- the checks ---------- */

static void run_checks( HPS hps )
{
    POINTL pt;
    LONG   before, once;

    GpiErase( hps );
    check( "erased pel is CLR_BACKGROUND", pel_at( hps, 5, 5 ), CLR_BACKGROUND );

    GpiSetColor( hps, CLR_RED );
    pt.x = 10; pt.y = 10;
    GpiSetPel( hps, &pt );
    check( "GpiSetPel then GpiQueryPel", pel_at( hps, 10, 10 ), CLR_RED );
    check( "neighbour untouched", pel_at( hps, 11, 10 ), CLR_BACKGROUND );

    GpiSetColor( hps, CLR_BLUE );
    box( hps, 20, 20, 40, 30 );
    check( "inside GpiBox", pel_at( hps, 30, 25 ), CLR_BLUE );
    check( "corner of GpiBox", pel_at( hps, 40, 30 ), CLR_BLUE );
    check( "outside GpiBox", pel_at( hps, 41, 25 ), CLR_BACKGROUND );

    GpiSetColor( hps, CLR_GREEN );
    pt.x = 50; pt.y = 10;
    GpiMove( hps, &pt );
    pt.y = 40;
    GpiLine( hps, &pt );
    check( "on GpiLine", pel_at( hps, 50, 25 ), CLR_GREEN );

    /* XOR: changes the pel, and a second XOR restores it. */
    check( "GpiSetMix(FM_XOR)", GpiSetMix( hps, FM_XOR ), TRUE );
    check( "GpiQueryMix", GpiQueryMix( hps ), FM_XOR );
    GpiSetColor( hps, CLR_YELLOW );
    before = pel_at( hps, 30, 25 );
    box( hps, 25, 22, 35, 28 );
    once = pel_at( hps, 30, 25 );
    check( "FM_XOR changes the pel", once != before, TRUE );
    box( hps, 25, 22, 35, 28 );
    check( "FM_XOR twice restores", pel_at( hps, 30, 25 ), before );

    /* Invert ignores the colour; twice restores. */
    GpiSetMix( hps, FM_INVERT );
    box( hps, 25, 22, 35, 28 );
    check( "FM_INVERT changes the pel", pel_at( hps, 30, 25 ) != before, TRUE );
    box( hps, 25, 22, 35, 28 );
    check( "FM_INVERT twice restores", pel_at( hps, 30, 25 ), before );

    GpiSetMix( hps, FM_LEAVEALONE );
    box( hps, 25, 22, 35, 28 );
    check( "FM_LEAVEALONE draws nothing", pel_at( hps, 30, 25 ), before );

    check( "GpiSetMix rejects 3", GpiSetMix( hps, 3 ), FALSE );
    GpiSetMix( hps, FM_OVERPAINT );

    check( "outside the window", pel_at( hps, -1, -1 ), CLR_NOINDEX );
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
            strcpy( line, "pm_pel_test: done " );
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

    WinRegisterClass( hab, "PMPelTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flFrameFlags,
                                    "PMPelTest", "Warpine GPI read-back test",
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
