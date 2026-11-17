/*
 * pm_destroy_test.c - WinDestroyWindow and WM_DESTROY under Warpine
 *
 * After start-up the client builds a second frame window and destroys it:
 *
 *   test frame
 *     test client           (class DestroyClient)
 *       child 1             (class DestroyChild)
 *         grandchild        (class DestroyChild)
 *       child 2             (class DestroyChild)
 *   owned window            (DestroyChild on the desktop, owned by the frame)
 *   modeless dialog         (WinLoadDlg, owned by the test client)
 *
 * Every window procedure records the WM_DESTROY it gets. Following the
 * WinDestroyWindow reference, the message goes first to the window being
 * destroyed and then to its children, while all of them still exist; the
 * frame's WM_DESTROY processing destroys the windows owned by the frame
 * and by its children. Child 2's handler also calls WinDestroyWindow on
 * itself and on its parent again, which must not send anything twice.
 *
 * Then: a WinGetPS presentation space is released with its window, a
 * destroyed handle cannot be destroyed again, a lone child window gets its
 * WM_DESTROY, and WinDlgBox destroys its dialog (with WM_DESTROY) before
 * it returns.
 *
 * Each check is written to stdout:
 *
 *   pm_destroy_test: <name>: ok
 *   pm_destroy_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_destroy_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#define INCL_GPI
#include <os2.h>
#include <string.h>
#include "pm_destroy_test.h"

#define WM_START   ( WM_USER + 1 )
#define MAX_LOG    32

static HAB  hab;
static int  n_checks = 0;
static int  n_passed = 0;

/* WM_DESTROY messages in the order they came. */
static HWND destroyed[MAX_LOG];
static int  n_destroyed = 0;

static HWND tframe, tclient, child1, child2, grand, owned, modeless;
static BOOL children_existed = FALSE;   /* seen from the client's WM_DESTROY */
static HWND parent_seen = NULLHANDLE;   /* WinQueryWindow( child1, QW_PARENT ) then */

static HWND modal_dlg = NULLHANDLE;
static int  modal_destroyed = 0;
static BOOL modal_button_existed = FALSE;

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
    char  line[160];
    char *p = append_str( line, "pm_destroy_test: " );
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

static void record( HWND hwnd )
{
    if( n_destroyed < MAX_LOG ) destroyed[n_destroyed++] = hwnd;
}

/* Position of hwnd's first WM_DESTROY, or -1. */
static int pos( HWND hwnd )
{
    int i;
    for( i = 0; i < n_destroyed; i++ )
        if( destroyed[i] == hwnd ) return i;
    return -1;
}

static int count( HWND hwnd )
{
    int i, n = 0;
    for( i = 0; i < n_destroyed; i++ )
        if( destroyed[i] == hwnd ) n++;
    return n;
}

/* ---------- the windows being destroyed ---------- */

MRESULT EXPENTRY TestClientProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_DESTROY ) {
        record( hwnd );
        children_existed = WinIsWindow( hab, child1 ) && WinIsWindow( hab, child2 )
                           && WinIsWindow( hab, grand );
        parent_seen = WinQueryWindow( child1, QW_PARENT );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

MRESULT EXPENTRY ChildProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_DESTROY ) {
        record( hwnd );
        if( hwnd == child2 ) {
            /* Destroying windows that are being destroyed already. */
            WinDestroyWindow( hwnd );
            WinDestroyWindow( tclient );
        }
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

MRESULT EXPENTRY ModelessProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_DESTROY ) {
        record( hwnd );
        return 0;
    }
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

MRESULT EXPENTRY ModalProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_INITDLG:
        modal_dlg = hwnd;
        WinDismissDlg( hwnd, 42 );
        return 0;
    case WM_DESTROY:
        modal_destroyed++;
        modal_button_existed = WinIsWindow( hab, WinWindowFromID( hwnd, ID_DLG_OK ) );
        return 0;
    }
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

/* ---------- the checks ---------- */

static void destroy_tree( void )
{
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    HPS   hps;
    BOOL  rc;

    tframe = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "DestroyClient",
                                 "Destroy me", 0, NULLHANDLE, 0, &tclient );
    child1 = WinCreateWindow( tclient, "DestroyChild", "", WS_VISIBLE,
                              10, 10, 50, 50, tclient, HWND_TOP, 1, NULL, NULL );
    grand  = WinCreateWindow( child1, "DestroyChild", "", WS_VISIBLE,
                              5, 5, 20, 20, child1, HWND_TOP, 3, NULL, NULL );
    child2 = WinCreateWindow( tclient, "DestroyChild", "", WS_VISIBLE,
                              70, 10, 50, 50, tclient, HWND_TOP, 2, NULL, NULL );
    owned  = WinCreateWindow( HWND_DESKTOP, "DestroyChild", "", 0,
                              0, 0, 10, 10, tframe, HWND_TOP, 4, NULL, NULL );
    modeless = WinLoadDlg( HWND_DESKTOP, tclient, ModelessProc, NULLHANDLE, ID_DLG, NULL );
    check( "the windows were created",
           tframe && tclient && child1 && grand && child2 && owned && modeless, TRUE );
    hps = WinGetPS( child1 );

    rc = WinDestroyWindow( tframe );
    check( "WinDestroyWindow returns TRUE", rc, TRUE );
    check( "the client got one WM_DESTROY", count( tclient ), 1 );
    check( "child 1 got one WM_DESTROY", count( child1 ), 1 );
    check( "child 2 got one WM_DESTROY", count( child2 ), 1 );
    check( "the grandchild got one WM_DESTROY", count( grand ), 1 );
    check( "the owned window got one WM_DESTROY", count( owned ), 1 );
    check( "the owned modeless dialog got one WM_DESTROY", count( modeless ), 1 );
    check( "no other WM_DESTROY (the frame has its own default)", n_destroyed, 6 );
    check( "the client before its children",
           pos( tclient ) < pos( child1 ) && pos( tclient ) < pos( child2 ), TRUE );
    check( "a child before its own child", pos( child1 ) < pos( grand ), TRUE );
    check( "the children existed during the client's WM_DESTROY", children_existed, TRUE );
    check( "WinQueryWindow worked during it", parent_seen == tclient, TRUE );
    check( "all windows are gone",
           WinIsWindow( hab, tframe ) || WinIsWindow( hab, tclient ) || WinIsWindow( hab, child1 )
           || WinIsWindow( hab, child2 ) || WinIsWindow( hab, grand ) || WinIsWindow( hab, owned )
           || WinIsWindow( hab, modeless ), FALSE );
    check( "a destroyed handle cannot be destroyed again", WinDestroyWindow( tframe ), FALSE );
    check( "the WinGetPS space was released with its window", WinReleasePS( hps ), FALSE );
}

static void destroy_child( HWND parent )
{
    HWND c = WinCreateWindow( parent, "DestroyChild", "", 0, 0, 0, 10, 10,
                              parent, HWND_TOP, 9, NULL, NULL );
    int  before = n_destroyed;
    check( "a lone child is destroyed", WinDestroyWindow( c ), TRUE );
    check( "and got WM_DESTROY", n_destroyed == before + 1 && destroyed[before] == c, TRUE );
    check( "and is gone", WinIsWindow( hab, c ), FALSE );
}

static void dialog_box( HWND owner )
{
    ULONG rc = WinDlgBox( HWND_DESKTOP, owner, ModalProc, NULLHANDLE, ID_DLG, NULL );
    check( "WinDlgBox returns the dismissal value", rc, 42 );
    check( "its dialog got one WM_DESTROY", modal_destroyed, 1 );
    check( "with its button still there", modal_button_existed, TRUE );
    check( "and is gone when WinDlgBox returns", WinIsWindow( hab, modal_dlg ), FALSE );
}

/* ---------- the main client ---------- */

MRESULT EXPENTRY MainWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_START:
        destroy_tree();
        destroy_child( hwnd );
        dialog_box( hwnd );
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;

    case WM_PAINT: {
        RECTL rc;
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinFillRect( hps, &rc, CLR_WHITE );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    HWND  frame, client;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "DestroyTest", MainWndProc, CS_SIZEREDRAW, 0 );
    WinRegisterClass( hab, "DestroyClient", TestClientProc, CS_SIZEREDRAW, 0 );
    WinRegisterClass( hab, "DestroyChild", ChildProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "DestroyTest",
                                "PM Destroy Test", 0, NULLHANDLE, 0, &client );
    WinPostMsg( client, WM_START, 0, 0 );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    p = append_str( line, "pm_destroy_test: done " );
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
