/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_frame_accel_test.c - frame controls, accelerator tables and other
 * window calls under Warpine
 *
 * Checks the PMWIN calls
 *
 *   - WinCreateStdWindow with FCF_VERTSCROLL: an FID_VERTSCROLL scroll bar
 *     that takes SBM_* messages, the client beside it
 *   - WinCalcFrameRect: frame rectangle -> client rectangle and back
 *   - WM_QUERYFRAMECTLCOUNT / WM_FORMATFRAME / WM_CALCFRAMERECT answered by
 *     WinDefWindowProc for the frame
 *   - WinCreateWindow(WC_FRAME) + WinCreateFrameControls (title bar,
 *     FCF_HORZSCROLL) + a client created with FID_CLIENT
 *   - WinCreateAccelTable / WinCopyAccelTable / WinSetAccelTable /
 *     WinQueryAccelTable / WinTranslateAccel / WinDestroyAccelTable, and
 *     WinCopyAccelTable of a table loaded from the ACCELTABLE resource
 *   - WinLoadMessage (MESSAGETABLE) and WinLoadString (STRINGTABLE)
 *   - WinQueryVersion, WinFlashWindow, WinSetWindowBits,
 *     WinSetMultWindowPos, WinSetDlgItemShort / WinQueryDlgItemShort
 *
 * Each check is written to stdout:
 *
 *   pm_frame_accel_test: <name>: ok
 *   pm_frame_accel_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_frame_accel_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_DOS
#include <os2.h>
#include <string.h>
#include "pm_frame_accel_test.h"

#define BAD_HWND  ( (HWND)0xDEAD )
#define ID_FIELD  30

static int n_checks = 0;
static int n_passed = 0;

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
    char  line[200];
    char *p = append_str( line, "pm_frame_accel_test: " );
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

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

/* An ACCELTABLE with two entries (ACCELTABLE declares one). */
#pragma pack( push, 2 )
static struct {
    USHORT cAccel;
    USHORT codepage;
    ACCEL  aaccel[2];
} table = { 2, 850, { { AF_VIRTUALKEY, VK_F3, 300 }, { AF_CHAR | AF_CONTROL, 's', 301 } } };
#pragma pack( pop )

int main( void )
{
    HAB    hab;
    HMQ    hmq;
    HWND   frame, client, vbar, wcframe, wcclient, hbar, field;
    ULONG  flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_VERTSCROLL;
    RECTL  r, rc;
    SWP    swp[4], pos;
    FRAMECDATA fcd;
    HACCEL haccel, hres;
    QMSG   qmsg;
    BYTE   copy[32];
    char   buf[32], line[64], *p;
    SHORT  s;
    LONG   n, i, found;
    ULONG  ver;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "FrameTest", ClientWndProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "FrameTest",
                                "Frame Test", WS_VISIBLE, NULLHANDLE, 0, &client );

    /* Frame scroll bar from FCF_VERTSCROLL. */
    vbar = WinWindowFromID( frame, FID_VERTSCROLL );
    check( "FCF_VERTSCROLL creates FID_VERTSCROLL", vbar != NULLHANDLE, TRUE );
    check( "no FID_HORZSCROLL without FCF_HORZSCROLL", WinWindowFromID( frame, FID_HORZSCROLL ), 0 );
    WinSendMsg( vbar, SBM_SETSCROLLBAR, MPFROMSHORT( 7 ), MPFROM2SHORT( 0, 20 ) );
    check( "the frame scroll bar takes SBM_SETSCROLLBAR",
           SHORT1FROMMR( WinSendMsg( vbar, SBM_QUERYPOS, 0, 0 ) ), 7 );
    WinQueryWindowPos( vbar, &pos );
    WinQueryWindowRect( client, &rc );
    check( "the client ends where the scroll bar starts", rc.xRight, pos.x );

    /* WinCalcFrameRect. */
    WinQueryWindowRect( frame, &r );
    check( "WinCalcFrameRect(frame -> client)", WinCalcFrameRect( frame, &r, TRUE ), TRUE );
    check( "-> client width", r.xRight - r.xLeft, rc.xRight );
    check( "-> client height", r.yTop - r.yBottom, rc.yTop );
    check( "WinCalcFrameRect(client -> frame)", WinCalcFrameRect( frame, &r, FALSE ), TRUE );
    WinQueryWindowPos( frame, &pos );
    check( "-> frame width", r.xRight - r.xLeft, pos.cx );
    check( "-> frame height", r.yTop - r.yBottom, pos.cy );
    WinSetRect( hab, &r, 0, 0, 100, 5 );
    check( "a frame too small for a client fails", WinCalcFrameRect( frame, &r, TRUE ), FALSE );
    check( "the client is no frame", WinCalcFrameRect( client, &r, TRUE ), FALSE );

    /* Frame formatting messages. */
    n = (LONG)WinSendMsg( frame, WM_QUERYFRAMECTLCOUNT, 0, 0 );
    check( "WM_QUERYFRAMECTLCOUNT counts scroll bar and client", n, 2 );
    memset( swp, 0, sizeof( swp ) );
    check( "WM_FORMATFRAME fills the SWPs",
           (LONG)WinSendMsg( frame, WM_FORMATFRAME, MPFROMP( swp ), MPFROMP( &r ) ), 2 );
    found = 0;
    for( i = 0; i < 2; i++ ) {
        if( swp[i].hwnd == client && swp[i].cx == rc.xRight && swp[i].cy == rc.yTop ) found = 1;
    }
    check( "-> the client's SWP", found, 1 );
    check( "-> the client rectangle", r.xRight - r.xLeft, rc.xRight );
    WinQueryWindowRect( frame, &r );
    check( "WM_CALCFRAMERECT", (LONG)WinSendMsg( frame, WM_CALCFRAMERECT, MPFROMP( &r ), MPFROMLONG( TRUE ) ), TRUE );
    check( "-> client height", r.yTop - r.yBottom, rc.yTop );

    /* WC_FRAME + WinCreateFrameControls + FID_CLIENT. */
    wcframe = WinCreateWindow( HWND_DESKTOP, WC_FRAME, "", WS_VISIBLE, 300, 100, 240, 160,
                               NULLHANDLE, HWND_TOP, 7, NULL, NULL );
    check( "WinCreateWindow(WC_FRAME)", wcframe != NULLHANDLE, TRUE );
    fcd.cb = sizeof( fcd );
    fcd.flCreateFlags = FCF_TITLEBAR | FCF_BORDER | FCF_HORZSCROLL;
    fcd.hmodResources = 0;
    fcd.idResources = 0;
    check( "WinCreateFrameControls", WinCreateFrameControls( wcframe, &fcd, "Controls" ), TRUE );
    WinQueryWindowText( wcframe, sizeof( buf ), buf );
    check( "-> the title", strcmp( buf, "Controls" ), 0 );
    hbar = WinWindowFromID( wcframe, FID_HORZSCROLL );
    check( "-> FID_HORZSCROLL", hbar != NULLHANDLE, TRUE );
    wcclient = WinCreateWindow( wcframe, "FrameTest", "", WS_VISIBLE, 0, 0, 0, 0,
                                wcframe, HWND_TOP, FID_CLIENT, NULL, NULL );
    WinQueryWindowPos( hbar, &pos );
    WinQueryWindowPos( wcclient, &swp[0] );
    check( "the FID_CLIENT window is laid out above the scroll bar", swp[0].y, pos.y + pos.cy );
    check( "-> its height", swp[0].cy, 160 - pos.y - pos.cy - WinQuerySysValue( HWND_DESKTOP, SV_CYTITLEBAR ) );
    fcd.cb = 4;
    check( "a short FRAMECDATA fails", WinCreateFrameControls( wcframe, &fcd, NULL ), FALSE );
    check( "-> PMERR_PARAMETER_OUT_OF_RANGE", ERRORIDERROR( WinGetLastError( hab ) ),
           PMERR_PARAMETER_OUT_OF_RANGE );

    /* Accelerator tables. */
    haccel = WinCreateAccelTable( hab, (PACCELTABLE)&table );
    check( "WinCreateAccelTable", haccel != NULLHANDLE, TRUE );
    check( "WinCopyAccelTable(NULL) is the size", WinCopyAccelTable( haccel, NULL, 0 ), sizeof( table ) );
    memset( copy, 0, sizeof( copy ) );
    check( "WinCopyAccelTable copies it", WinCopyAccelTable( haccel, (PACCELTABLE)copy, sizeof( copy ) ),
           sizeof( table ) );
    check( "-> the same table", memcmp( copy, &table, sizeof( table ) ), 0 );
    check( "WinCopyAccelTable stops at cbCopyMax", WinCopyAccelTable( haccel, (PACCELTABLE)copy, 4 ), 4 );
    check( "WinSetAccelTable(frame)", WinSetAccelTable( hab, haccel, frame ), TRUE );
    check( "WinQueryAccelTable(frame)", WinQueryAccelTable( hab, frame ) == haccel, TRUE );
    qmsg.hwnd = client;
    qmsg.msg = WM_CHAR;
    qmsg.mp1 = MPFROMSH2CH( KC_VIRTUALKEY, 1, 0 );
    qmsg.mp2 = MPFROM2SHORT( 0, VK_F3 );
    check( "WinTranslateAccel with the created table", WinTranslateAccel( hab, client, haccel, &qmsg ), TRUE );
    check( "-> WM_COMMAND", qmsg.msg, WM_COMMAND );
    check( "-> its command", SHORT1FROMMP( qmsg.mp1 ), 300 );
    check( "WinDestroyAccelTable", WinDestroyAccelTable( haccel ), TRUE );
    check( "-> no longer set for the frame", WinQueryAccelTable( hab, frame ), 0 );
    check( "WinDestroyAccelTable again fails", WinDestroyAccelTable( haccel ), FALSE );
    check( "-> PMERR_INVALID_HACCEL", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INVALID_HACCEL );
    hres = WinLoadAccelTable( hab, NULLHANDLE, ID_ACCEL );
    check( "WinLoadAccelTable", hres != NULLHANDLE, TRUE );
    WinCopyAccelTable( hres, (PACCELTABLE)copy, sizeof( copy ) );
    check( "-> cAccel of the resource", ((PACCELTABLE)copy)->cAccel, 1 );
    check( "-> its command", ((PACCELTABLE)copy)->aaccel[0].cmd, IDM_REFRESH );

    /* WinLoadMessage / WinLoadString. */
    n = WinLoadMessage( hab, NULLHANDLE, MSG_HELLO, sizeof( buf ), buf );
    check( "WinLoadMessage", n, 9 );
    check( "-> the message", strcmp( buf, "Seventeen" ), 0 );
    check( "WinLoadMessage cut to cchMax - 1", WinLoadMessage( hab, NULLHANDLE, MSG_HELLO, 4, buf ), 3 );
    check( "-> NUL-terminated", strcmp( buf, "Sev" ), 0 );
    n = WinLoadString( hab, NULLHANDLE, STR_FIVE, sizeof( buf ), buf );
    check( "WinLoadString", n, 11 );
    check( "-> the string", strcmp( buf, "String five" ), 0 );
    check( "WinLoadMessage of a missing message", WinLoadMessage( hab, NULLHANDLE, 99, sizeof( buf ), buf ), 0 );

    /* WinQueryVersion. */
    ver = WinQueryVersion( hab );
    check( "WinQueryVersion: version 20", ver & 0xFF, 20 );
    check( "WinQueryVersion: release 45", ( ver >> 8 ) & 0xFF, 45 );

    /* WinFlashWindow. */
    check( "WinFlashWindow(TRUE)", WinFlashWindow( frame, TRUE ), TRUE );
    check( "WinFlashWindow(FALSE)", WinFlashWindow( frame, FALSE ), TRUE );
    check( "WinFlashWindow(bad hwnd)", WinFlashWindow( BAD_HWND, TRUE ), FALSE );

    /* WinSetWindowBits. */
    check( "WinSetWindowBits(WS_DISABLED)",
           WinSetWindowBits( client, QWL_STYLE, WS_DISABLED | 1, WS_DISABLED ), TRUE );
    check( "-> disabled", WinIsWindowEnabled( client ), FALSE );
    check( "-> unmasked bit unchanged", WinQueryWindowULong( client, QWL_STYLE ) & 1, 0 );
    WinSetWindowBits( client, QWL_STYLE, 0, WS_DISABLED );
    check( "-> enabled again", WinIsWindowEnabled( client ), TRUE );

    /* WinSetMultWindowPos. */
    memset( swp, 0, sizeof( swp ) );
    swp[0].hwnd = frame;  swp[0].fl = SWP_MOVE | SWP_SIZE;
    swp[0].x = 40; swp[0].y = 60; swp[0].cx = 420; swp[0].cy = 300;
    swp[1].hwnd = wcframe; swp[1].fl = SWP_MOVE;
    swp[1].x = 500; swp[1].y = 400;
    check( "WinSetMultWindowPos", WinSetMultWindowPos( hab, swp, 2 ), TRUE );
    WinQueryWindowPos( frame, &pos );
    check( "-> frame moved", pos.x * 10000 + pos.y, 40 * 10000 + 60 );
    check( "-> frame sized", pos.cx * 10000 + pos.cy, 420 * 10000 + 300 );
    WinQueryWindowPos( wcframe, &pos );
    check( "-> second frame moved", pos.x * 10000 + pos.y, 500 * 10000 + 400 );
    WinQueryWindowRect( client, &rc );
    WinQueryWindowRect( frame, &r );
    WinCalcFrameRect( frame, &r, TRUE );
    check( "-> client follows the frame", r.xRight - r.xLeft, rc.xRight );
    swp[1].hwnd = BAD_HWND;
    check( "WinSetMultWindowPos(bad hwnd) fails", WinSetMultWindowPos( hab, swp, 2 ), FALSE );

    /* WinSetDlgItemShort / WinQueryDlgItemShort. */
    field = WinCreateWindow( client, WC_ENTRYFIELD, "", WS_VISIBLE, 10, 10, 80, 20,
                             client, HWND_TOP, ID_FIELD, NULL, NULL );
    check( "WinSetDlgItemShort(signed)", WinSetDlgItemShort( client, ID_FIELD, (USHORT)-42, TRUE ), TRUE );
    WinQueryWindowText( field, sizeof( buf ), buf );
    check( "-> text -42", strcmp( buf, "-42" ), 0 );
    s = 0;
    check( "WinQueryDlgItemShort(signed)", WinQueryDlgItemShort( client, ID_FIELD, &s, TRUE ), TRUE );
    check( "-> -42", s, -42 );
    check( "WinQueryDlgItemShort(unsigned) of a negative number",
           WinQueryDlgItemShort( client, ID_FIELD, &s, FALSE ), FALSE );
    WinSetDlgItemShort( client, ID_FIELD, 40000, FALSE );
    WinQueryWindowText( field, sizeof( buf ), buf );
    check( "WinSetDlgItemShort(unsigned) -> text 40000", strcmp( buf, "40000" ), 0 );
    WinSetWindowText( field, "12x" );
    check( "WinQueryDlgItemShort of no number", WinQueryDlgItemShort( client, ID_FIELD, &s, TRUE ), FALSE );

    WinDestroyWindow( wcframe );
    WinDestroyWindow( frame );

    p = append_str( line, "pm_frame_accel_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
