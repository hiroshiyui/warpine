/*
 * pm_child_dialog_test.c - a dialog embedded in a client window
 *
 * A standard frame (title bar, sizing border) is placed at desktop
 * (100, 200), 400 x 300; its client fills itself pale yellow. The program
 * loads a dialog with WinLoadDlg whose parent is the client: as in PM it
 * is a child window of the client, clipped to it, without a frame of its
 * own (ADR-0042). The dialog is placed at client (20, 20), 300 x 200; its
 * entry field at dialog (10, 150), 200 x 24 and its push button "Press"
 * at (10, 10), 100 x 30. The integration test (screen y = 767 - PM y; the
 * client's (0, 0) is screen (102, 567)):
 *
 *   clicks the button (screen 182, 522): the dialog procedure gets
 *   WM_COMMAND(ID_PRESS);
 *   clicks the entry field (232, 385) and types "abc";
 *   clicks the dialog's own area at its (250, 100) (screen 372, 447): the
 *   dialog procedure gets WM_BUTTON1DOWN there;
 *   closes the window with Alt+F4.
 *
 * Checked: the dialog's parent, position and visibility, that it did not
 * become the active window, the focus and text of the entry field, the
 * messages above, and that destroying the frame destroys the dialog (one
 * WM_DESTROY). Each check is written to stdout:
 *
 *   pm_child_dialog_test: <name>: ok
 *   pm_child_dialog_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_child_dialog_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#include <os2.h>
#include <string.h>
#include "pm_child_dialog_test.h"

static HAB  hab;
static HWND frame, client, dlg;
static int  n_checks = 0;
static int  n_passed = 0;

static int  pressed = 0;
static LONG click_x = -1, click_y = -1;
static int  destroyed = 0;

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
    char *p = append_str( append_str( line, "pm_child_dialog_test: " ), what );
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

static MRESULT EXPENTRY ChildDlgProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_COMMAND:
        /* Not WinDefDlgProc: that would dismiss (hide) the dialog. */
        if( SHORT1FROMMP( mp1 ) == ID_PRESS ) pressed++;
        return 0;
    case WM_BUTTON1DOWN:
        click_x = SHORT1FROMMP( mp1 );
        click_y = SHORT2FROMMP( mp1 );
        break;
    case WM_DESTROY:
        destroyed++;
        break;
    }
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

static void final_checks( void )
{
    char text[32];
    HWND entry = WinWindowFromID( dlg, ID_NAME );

    check( "the button's WM_COMMAND reached the dialog", pressed, 1 );
    check( "the click on the dialog reached it at x", click_x, 250 );
    check( "... and y", click_y, 100 );
    check( "the entry field has the focus", WinQueryFocus( HWND_DESKTOP ) == entry, TRUE );
    WinQueryWindowText( entry, sizeof( text ), text );
    check( "the typed text is in the entry field", strcmp( text, "abc" ) == 0, TRUE );
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
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    SWP   swp;
    POINTL pt;
    char  line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "ChildDlgTest", ClientProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "ChildDlgTest",
                                "Embedded dialog", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );

    dlg = WinLoadDlg( client, client, ChildDlgProc, NULLHANDLE, ID_CHILD, NULL );
    check( "WinLoadDlg", dlg != NULLHANDLE, TRUE );
    WinSetWindowPos( dlg, HWND_TOP, 20, 20, 300, 200, SWP_MOVE | SWP_SIZE );
    WinSetWindowPos( WinWindowFromID( dlg, ID_NAME ), HWND_TOP, 10, 150, 200, 24, SWP_MOVE | SWP_SIZE );
    WinSetWindowPos( WinWindowFromID( dlg, ID_PRESS ), HWND_TOP, 10, 10, 100, 30, SWP_MOVE | SWP_SIZE );

    check( "its parent is the client", WinQueryWindow( dlg, QW_PARENT ) == client, TRUE );
    WinQueryWindowPos( dlg, &swp );
    check( "placed relative to the client: x", swp.x, 20 );
    check( "... y", swp.y, 20 );
    check( "visible (WS_VISIBLE in the template)", WinIsWindowVisible( dlg ), TRUE );
    pt.x = 0; pt.y = 0;
    WinMapWindowPoints( dlg, client, &pt, 1 );
    check( "its origin in client coordinates", pt.x * 1000 + pt.y, 20 * 1000 + 20 );
    check( "the frame stays the active window", WinQueryActiveWindow( HWND_DESKTOP ) == frame, TRUE );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( frame );
    check( "destroying the frame destroyed the dialog", destroyed, 1 );
    check( "... and its handle is gone", WinIsWindow( hab, dlg ), FALSE );

    append_str( append_long( append_str( append_long( append_str( line, "pm_child_dialog_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
