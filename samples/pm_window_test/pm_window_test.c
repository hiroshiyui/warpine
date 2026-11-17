/*
 * pm_window_test.c - window tree and window-word calls under Warpine
 *
 * Checks the PMWIN calls
 *
 *   - WinIsWindowVisible: TRUE only when the window and all its parents
 *     have WS_VISIBLE
 *   - WinQueryWindowTextLength: the text length without the null
 *   - WinSetParent: moves a window under another parent (WinQueryWindow
 *     QW_PARENT / QW_TOP follow), to the desktop and back; a descendant as
 *     the new parent fails with PMERR_INVALID_HWND
 *   - WinSetOwner: sets and clears (NULLHANDLE) the owner
 *   - WinQueryWindowPtr / WinSetWindowPtr: the application's window words
 *     (shared with WinQueryWindowULong), QWP_PFNWP for the window
 *     procedure, PMERR_PARAMETER_OUT_OF_RANGE past cbWindowData - 4
 *
 * Each check is written to stdout:
 *
 *   pm_window_test: <name>: ok
 *   pm_window_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_window_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#include <os2.h>
#include <string.h>

#define BAD_HWND  ( (HWND)0xDEAD )
/* Bytes of window words the client class reserves. */
#define CB_WINDOW_DATA 8

static int   n_checks = 0;
static int   n_passed = 0;
static ULONG some_data = 42;

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
    char *p = append_str( line, "pm_window_test: " );
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

static MRESULT EXPENTRY OtherWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HAB   hab;
    HMQ   hmq;
    HWND  frame, client, frame2, client2, label, hidden;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "WindowTest", ClientWndProc, 0, CB_WINDOW_DATA );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "WindowTest",
                                "Window Test", WS_VISIBLE, NULLHANDLE, 0, &client );
    frame2 = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "WindowTest",
                                 "Second", WS_VISIBLE, NULLHANDLE, 0, &client2 );
    label = WinCreateWindow( client, WC_STATIC, "Label", WS_VISIBLE | SS_TEXT,
                             10, 10, 100, 20, client, HWND_TOP, 1, NULL, NULL );
    hidden = WinCreateWindow( client, WC_STATIC, "Hidden", SS_TEXT,
                              10, 40, 100, 20, client, HWND_TOP, 2, NULL, NULL );

    /* WinIsWindowVisible: the window and all its parents. The clients were
       created with WS_VISIBLE in flClientStyle. */
    check( "WinIsWindowVisible(frame)", WinIsWindowVisible( frame ), TRUE );
    check( "WinIsWindowVisible(label)", WinIsWindowVisible( label ), TRUE );
    check( "WinIsWindowVisible(hidden)", WinIsWindowVisible( hidden ), FALSE );
    WinShowWindow( frame, FALSE );
    check( "label of a hidden frame is not visible", WinIsWindowVisible( label ), FALSE );
    WinShowWindow( frame, TRUE );
    check( "visible again with its frame", WinIsWindowVisible( label ), TRUE );
    check( "WinIsWindowVisible(bad hwnd)", WinIsWindowVisible( BAD_HWND ), FALSE );
    check( "-> PMERR_INVALID_HWND", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INVALID_HWND );

    /* WinQueryWindowTextLength: without the null. */
    check( "WinQueryWindowTextLength(frame)", WinQueryWindowTextLength( frame ), 11 );
    check( "WinQueryWindowTextLength(label)", WinQueryWindowTextLength( label ), 5 );
    WinSetWindowText( label, "" );
    check( "empty text has length 0", WinQueryWindowTextLength( label ), 0 );

    /* WinSetParent. */
    check( "WinSetParent(label, client2)", WinSetParent( label, client2, TRUE ), TRUE );
    check( "-> QW_PARENT is client2", WinQueryWindow( label, QW_PARENT ) == client2, TRUE );
    check( "-> on top of client2's children", WinQueryWindow( client2, QW_TOP ) == label, TRUE );
    check( "-> left client", WinQueryWindow( client, QW_TOP ) == hidden, TRUE );
    check( "WinSetParent(label, HWND_DESKTOP)", WinSetParent( label, HWND_DESKTOP, FALSE ), TRUE );
    check( "-> QW_PARENT is the desktop", WinQueryWindow( label, QW_PARENT ) == HWND_DESKTOP, TRUE );
    check( "WinSetParent(label, client)", WinSetParent( label, client, TRUE ), TRUE );
    check( "WinSetParent(frame, its client) fails", WinSetParent( frame, client, TRUE ), FALSE );
    check( "-> PMERR_INVALID_HWND", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INVALID_HWND );
    check( "-> frame still on the desktop", WinQueryWindow( frame, QW_PARENT ) == HWND_DESKTOP, TRUE );

    /* WinSetOwner. */
    check( "WinSetOwner(frame2, frame)", WinSetOwner( frame2, frame ), TRUE );
    check( "-> QW_OWNER is frame", WinQueryWindow( frame2, QW_OWNER ) == frame, TRUE );
    check( "WinSetOwner(frame2, NULLHANDLE)", WinSetOwner( frame2, NULLHANDLE ), TRUE );
    check( "-> disowned", WinQueryWindow( frame2, QW_OWNER ), 0 );
    check( "WinSetOwner(frame2, bad hwnd)", WinSetOwner( frame2, BAD_HWND ), FALSE );
    check( "-> PMERR_INVALID_HWND", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INVALID_HWND );

    /* WinQueryWindowPtr / WinSetWindowPtr. */
    check( "WinSetWindowPtr(client, 4)", WinSetWindowPtr( client, 4, &some_data ), TRUE );
    check( "-> WinQueryWindowPtr reads it", WinQueryWindowPtr( client, 4 ) == (PVOID)&some_data, TRUE );
    check( "-> the same window words as WinQueryWindowULong",
           WinQueryWindowULong( client, 4 ) == (ULONG)&some_data, TRUE );
    check( "QWP_PFNWP is the window procedure",
           WinQueryWindowPtr( client, QWP_PFNWP ) == (PVOID)ClientWndProc, TRUE );
    check( "WinSetWindowPtr(QWP_PFNWP)", WinSetWindowPtr( client2, QWP_PFNWP, (PVOID)OtherWndProc ), TRUE );
    check( "-> procedure replaced",
           WinQueryWindowPtr( client2, QWP_PFNWP ) == (PVOID)OtherWndProc, TRUE );
    check( "WinQueryWindowPtr past cbWindowData - 4",
           (LONG)WinQueryWindowPtr( client, CB_WINDOW_DATA - 2 ), 0 );
    check( "-> PMERR_PARAMETER_OUT_OF_RANGE", ERRORIDERROR( WinGetLastError( hab ) ),
           PMERR_PARAMETER_OUT_OF_RANGE );
    check( "WinSetWindowPtr(bad hwnd)", WinSetWindowPtr( BAD_HWND, 0, &some_data ), FALSE );
    check( "-> PMERR_INVALID_HWND", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INVALID_HWND );

    WinDestroyWindow( frame2 );
    WinDestroyWindow( frame );

    p = append_str( line, "pm_window_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
