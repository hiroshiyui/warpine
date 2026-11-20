/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_embdlg_test.c - an embedded dialog's chrome, Tab across its edge, and
 * the pointer reaching a window of the program's own class inside it
 *
 * A standard frame (title bar, sizing border) is placed at desktop
 * (100, 200), 400 x 300; its client fills itself pale grey and holds an
 * entry field at client (10, 240), 150 x 24, and a push button "Out" at
 * (200, 240), 100 x 28, both tab stops. The program loads a dialog with
 * WinLoadDlg whose parent is the client (ADR-0042); its template asks for
 * a title bar and a dialog border (ADR-0069). It is placed at client
 * (20, 20), 300 x 200, with its entry field at dialog (10, 120), 200 x 24,
 * its push button "Press" at (10, 10), 100 x 30, and a "Pad" window of the
 * program's own class at (150, 10), 100 x 60, which fills itself blue.
 *
 * The integration test (screen y = 567 - client y; the client's (0, 0) is
 * screen (102, 567)):
 *
 *   clicks the pad at its (20, 30) (screen 292, 507): the pad gets
 *   WM_BUTTON1DOWN there;
 *   clicks the dialog's title bar at the dialog's (100, 190) (screen
 *   222, 357): the dialog procedure gets WM_BUTTON1DOWN there;
 *   clicks the client's entry field (screen 162, 315);
 *   Tab, Tab (the client's button, then the dialog's entry field), types
 *   "xy";
 *   Tab, Tab (the dialog's button, then back to the client's entry
 *   field), types "q";
 *   Shift+Tab (back into the dialog, its button) and Space: the dialog
 *   procedure gets WM_COMMAND(ID_PRESS);
 *   closes the window with Alt+F4.
 *
 * Checked before the input: WinWindowFromPoint finds the pad from the
 * desktop and the dialog from the client. Each check is written to stdout:
 *
 *   pm_embdlg_test: <name>: ok
 *   pm_embdlg_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_embdlg_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#include <os2.h>
#include <string.h>
#include "pm_embdlg_test.h"

static HAB  hab;
static HWND frame, client, dlg, pad, outer, outbtn;
static int  n_checks = 0;
static int  n_passed = 0;

static int  pressed = 0;
static LONG dlg_click_x = -1, dlg_click_y = -1;
static LONG pad_click_x = -1, pad_click_y = -1;

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
    char  line[160];
    char *p = append_str( append_str( line, "pm_embdlg_test: " ), what );
    if( got == want ) {
        append_str( p, ": ok\r\n" );
        n_passed++;
    } else {
        p = append_long( append_str( p, ": FAIL got " ), got );
        append_str( append_long( append_str( p, " want " ), want ), "\r\n" );
    }
    write_stdout( line );
    n_checks++;
}

static MRESULT EXPENTRY PadProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PAINT: {
        RECTL r;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &r );
        WinQueryWindowRect( hwnd, &r );
        WinFillRect( hps, &r, CLR_BLUE );
        WinEndPaint( hps );
        return 0;
    }
    case WM_BUTTON1DOWN:
        pad_click_x = SHORT1FROMMP( mp1 );
        pad_click_y = SHORT2FROMMP( mp1 );
        return (MRESULT)TRUE;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static MRESULT EXPENTRY InnerDlgProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_COMMAND:
        /* Not WinDefDlgProc: that would dismiss (hide) the dialog. */
        if( SHORT1FROMMP( mp1 ) == ID_PRESS ) pressed++;
        return 0;
    case WM_BUTTON1DOWN:
        dlg_click_x = SHORT1FROMMP( mp1 );
        dlg_click_y = SHORT2FROMMP( mp1 );
        break;
    }
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

static void text_is( const char *what, HWND hwnd, const char *want )
{
    char text[32];
    WinQueryWindowText( hwnd, sizeof( text ), text );
    check( what, strcmp( text, want ) == 0, TRUE );
}

static void final_checks( void )
{
    check( "the click on the pad reached it at x", pad_click_x, 20 );
    check( "... and y", pad_click_y, 30 );
    check( "the click on the title bar reached the dialog at x", dlg_click_x, 100 );
    check( "... and y", dlg_click_y, 190 );
    text_is( "Tab went from the client into the dialog's entry field", WinWindowFromID( dlg, ID_NAME ), "xy" );
    text_is( "Tab went from the dialog back to the client's entry field", outer, "q" );
    check( "Shift+Tab went back into the dialog: its button was pressed", pressed, 1 );
    check( "the dialog's button has the focus",
           WinQueryFocus( HWND_DESKTOP ) == WinWindowFromID( dlg, ID_PRESS ), TRUE );
    check( "the frame is still the active window", WinQueryActiveWindow( HWND_DESKTOP ) == frame, TRUE );
}

static MRESULT EXPENTRY ClientProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PAINT: {
        RECTL r;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &r );
        WinQueryWindowRect( hwnd, &r );
        WinFillRect( hps, &r, CLR_PALEGRAY );
        WinEndPaint( hps );
        return 0;
    }
    case WM_CLOSE:
        final_checks();
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ    hmq;
    QMSG   qmsg;
    ULONG  flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    POINTL pt;
    char   line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "EmbDlgTest", ClientProc, CS_SIZEREDRAW, 0 );
    WinRegisterClass( hab, "EmbDlgPad", PadProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "EmbDlgTest",
                                "Embedded dialog chrome", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );

    outer = WinCreateWindow( client, WC_ENTRYFIELD, "", WS_VISIBLE | WS_TABSTOP | ES_MARGIN,
                             10, 240, 150, 24, client, HWND_TOP, ID_OUTER, NULL, NULL );
    outbtn = WinCreateWindow( client, WC_BUTTON, "Out", WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                              200, 240, 100, 28, client, HWND_TOP, ID_OUTBTN, NULL, NULL );

    dlg = WinLoadDlg( client, client, InnerDlgProc, NULLHANDLE, ID_DLG, NULL );
    check( "WinLoadDlg", dlg != NULLHANDLE, TRUE );
    WinSetWindowPos( dlg, HWND_TOP, 20, 20, 300, 200, SWP_MOVE | SWP_SIZE );
    WinSetWindowPos( WinWindowFromID( dlg, ID_NAME ), HWND_TOP, 10, 120, 200, 24, SWP_MOVE | SWP_SIZE );
    WinSetWindowPos( WinWindowFromID( dlg, ID_PRESS ), HWND_TOP, 10, 10, 100, 30, SWP_MOVE | SWP_SIZE );
    pad = WinCreateWindow( dlg, "EmbDlgPad", "", WS_VISIBLE, 150, 10, 100, 60,
                           dlg, HWND_TOP, ID_PAD, NULL, NULL );
    check( "the pad was created in the dialog", WinQueryWindow( pad, QW_PARENT ) == dlg, TRUE );

    /* The pad's (20, 30) on the desktop: the pointer finds the pad. */
    pt.x = 20; pt.y = 30;
    WinMapWindowPoints( pad, HWND_DESKTOP, &pt, 1 );
    check( "WinWindowFromPoint from the desktop finds the pad",
           WinWindowFromPoint( HWND_DESKTOP, &pt, TRUE ) == pad, TRUE );
    /* The client's (40, 100), in the dialog: its child there is the dialog. */
    pt.x = 40; pt.y = 100;
    check( "WinWindowFromPoint in the client finds the dialog",
           WinWindowFromPoint( client, &pt, FALSE ) == dlg, TRUE );
    pt.x = 40; pt.y = 250;
    check( "... and the client's entry field above it",
           WinWindowFromPoint( client, &pt, FALSE ) == outer, TRUE );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( frame );
    append_str( append_long( append_str( append_long( append_str( line, "pm_embdlg_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
