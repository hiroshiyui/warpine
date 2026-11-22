/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_error_test.c - WinGetLastError and per-thread PMERR_* codes under Warpine
 *
 * Makes PM calls fail on purpose and reads the error back with
 * WinGetLastError:
 *
 *   - a second WinCreateMsgQueue on a thread  -> PMERR_MSG_QUEUE_ALREADY_EXISTS
 *   - WinDestroyWindow / WinSetWindowPos / WinSetWindowText /
 *     WinQueryWindowText / WinWindowFromID / WinPostMsg / WinSendMsg with a
 *     handle that is no window               -> PMERR_INVALID_HWND
 *   - WinDestroyMsgQueue of another thread's queue -> PMERR_INVALID_HMQ
 *   - WinPeekMsg on a thread without a queue -> PMERR_NO_MSG_QUEUE
 *   - WinShowWindow, WinQueryWindowRect / Pos, the window words,
 *     WinEnableWindow, WinInvalidateRect, WinSetFocus, WinQueryWindow, the
 *     dialog-item calls, WinCreateWindow, WinPeekMsg / WinGetMsg filters ...
 *     with a handle that is no window       -> PMERR_INVALID_HWND
 *   - WinQueryWindow lCode, window words outside cbWindowData or below
 *     QWL_MIN                              -> PMERR_PARAMETER_OUT_OF_RANGE
 *   - WinPeekMsg with an unknown fl bit    -> PMERR_INVALID_FLAG
 *   - WinSetAccelTable with no such table  -> PMERR_INVALID_HACCEL
 *   - WinInvalidateRect with a flag that is no BOOL -> PMERR_INVALID_FLAG
 *   - WinBeginPaint with an HPS that is none -> PMERR_INV_HPS
 *   - WinSetDlgItemText / WinQueryDlgItemText of no such item -> PMERR_INVALID_HWND
 *   - WinCreateWindow on a thread without a queue -> PMERR_NO_MSG_QUEUE
 *   - WinSetSysValue(SV_CURSORRATE, 0)       -> PMERR_PARAMETER_OUT_OF_RANGE
 *     (and with a handle that is no desktop  -> PMERR_INVALID_HWND), after
 *     setting the caret blink rate and reading it back
 *
 * and checks that the value carries SEVERITY_ERROR in its high word, that
 * reading resets it to zero, that a later successful call leaves it alone,
 * and that each thread has its own last error.
 *
 * Each check is written to stdout:
 *
 *   pm_error_test: <name>: ok
 *   pm_error_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_error_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#define INCL_GPIERRORS
#define INCL_DOSPROCESS
#define INCL_DOSSEMAPHORES
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define BAD_HWND  ( (HWND)0xDEAD )

static HMQ   main_hmq;
static int   n_checks = 0;
static int   n_passed = 0;

/* What the second thread saw, checked by the main thread afterwards. */
static ULONG thr_destroy_rc;
static ERRORID thr_destroy_err;
static ULONG thr_peek_rc;
static ERRORID thr_peek_err;
static ULONG   thr_create_rc;
static ERRORID thr_create_err;
static ERRORID thr_initial_err;
static HEV   thr_done;

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
    char *p = append_str( line, "pm_error_test: " );
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

/* A failing call: its result and the PMERR_* code it left behind. */
static void check_fails( HAB hab, const char *what, ULONG rc, USHORT code )
{
    char  name[96];
    char *p = append_str( name, what );
    check( name, rc, 0 );
    append_str( p, " -> error code" );
    check( name, ERRORIDERROR( WinGetLastError( hab ) ), code );
}

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

/* ---------- the second thread ---------- */

static VOID APIENTRY second_thread( ULONG arg )
{
    HAB  hab = WinInitialize( 0 );
    QMSG qmsg;

    (void)arg;
    thr_initial_err = WinGetLastError( hab );
    thr_destroy_rc  = WinDestroyMsgQueue( main_hmq );
    thr_destroy_err = WinGetLastError( hab );
    thr_peek_rc     = WinPeekMsg( hab, &qmsg, NULLHANDLE, 0, 0, PM_REMOVE );
    thr_peek_err    = WinGetLastError( hab );
    thr_create_rc   = (ULONG)WinCreateWindow( HWND_DESKTOP, WC_STATIC, "x", 0, 0, 0, 10, 10,
                                              NULLHANDLE, HWND_TOP, 1, NULL, NULL );
    thr_create_err  = WinGetLastError( hab );
    WinTerminate( hab );
    DosPostEventSem( thr_done );
}

int main( void )
{
    HAB     hab;
    HWND    frame, client;
    ULONG   flags = FCF_TITLEBAR | FCF_SYSMENU;
    ERRORID err;
    TID     tid;
    char    text[16];
    char    line[64], *p;
    RECTL   rcl;
    SWP     swp;
    QMSG    qmsg;

    hab = WinInitialize( 0 );
    check( "no error at start", WinGetLastError( hab ), 0 );

    main_hmq = WinCreateMsgQueue( hab, 0 );
    check( "first WinCreateMsgQueue succeeds", main_hmq != NULLHANDLE, TRUE );
    check( "a success records no error", WinGetLastError( hab ), 0 );

    check( "second WinCreateMsgQueue fails", WinCreateMsgQueue( hab, 0 ), 0 );
    err = WinGetLastError( hab );
    check( "-> PMERR_MSG_QUEUE_ALREADY_EXISTS", ERRORIDERROR( err ), PMERR_MSG_QUEUE_ALREADY_EXISTS );
    check( "-> severity SEVERITY_ERROR", ERRORIDSEV( err ), SEVERITY_ERROR );
    check( "reading resets the error", WinGetLastError( hab ), 0 );

    WinRegisterClass( hab, "ErrorTest", ClientWndProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "ErrorTest",
                                "PM Error Test", 0, NULLHANDLE, 0, &client );

    check_fails( hab, "WinDestroyWindow(bad hwnd)", WinDestroyWindow( BAD_HWND ), PMERR_INVALID_HWND );
    check_fails( hab, "WinSetWindowPos(bad hwnd)",
                 WinSetWindowPos( BAD_HWND, HWND_TOP, 0, 0, 10, 10, SWP_SIZE ), PMERR_INVALID_HWND );
    check_fails( hab, "WinSetWindowText(bad hwnd)", WinSetWindowText( BAD_HWND, "x" ), PMERR_INVALID_HWND );
    check_fails( hab, "WinQueryWindowText(bad hwnd)",
                 WinQueryWindowText( BAD_HWND, sizeof( text ), text ), PMERR_INVALID_HWND );
    check_fails( hab, "WinWindowFromID(bad parent)", WinWindowFromID( BAD_HWND, 1 ), PMERR_INVALID_HWND );
    check_fails( hab, "WinPostMsg(bad hwnd)", WinPostMsg( BAD_HWND, WM_USER, 0, 0 ), PMERR_INVALID_HWND );
    check_fails( hab, "WinSendMsg(bad hwnd)", (ULONG)WinSendMsg( BAD_HWND, WM_USER, 0, 0 ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinDestroyMsgQueue(bad hmq)", WinDestroyMsgQueue( (HMQ)0xDEAD ), PMERR_INVALID_HMQ );

    /* More calls that take a window handle. */
    check_fails( hab, "WinShowWindow(bad hwnd)", WinShowWindow( BAD_HWND, TRUE ), PMERR_INVALID_HWND );
    check_fails( hab, "WinQueryWindowRect(bad hwnd)", WinQueryWindowRect( BAD_HWND, &rcl ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinQueryWindowPos(bad hwnd)", WinQueryWindowPos( BAD_HWND, &swp ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinSetWindowULong(bad hwnd)", WinSetWindowULong( BAD_HWND, QWL_USER, 1 ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinQueryWindowULong(bad hwnd)", WinQueryWindowULong( BAD_HWND, QWL_STYLE ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinSetWindowUShort(bad hwnd)", WinSetWindowUShort( BAD_HWND, QWS_USER, 1 ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinQueryWindowUShort(bad hwnd)", WinQueryWindowUShort( BAD_HWND, QWS_ID ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinEnableWindow(bad hwnd)", WinEnableWindow( BAD_HWND, TRUE ), PMERR_INVALID_HWND );
    check_fails( hab, "WinIsWindowEnabled(bad hwnd)", WinIsWindowEnabled( BAD_HWND ), PMERR_INVALID_HWND );
    check_fails( hab, "WinInvalidateRect(bad hwnd)", WinInvalidateRect( BAD_HWND, NULL, FALSE ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinUpdateWindow(bad hwnd)", WinUpdateWindow( BAD_HWND ), PMERR_INVALID_HWND );
    check_fails( hab, "WinSetFocus(bad hwnd)", WinSetFocus( HWND_DESKTOP, BAD_HWND ), PMERR_INVALID_HWND );
    check_fails( hab, "WinSetActiveWindow(bad hwnd)", WinSetActiveWindow( HWND_DESKTOP, BAD_HWND ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinSetCapture(bad hwnd)", WinSetCapture( HWND_DESKTOP, BAD_HWND ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinQueryWindow(bad hwnd)", WinQueryWindow( BAD_HWND, QW_PARENT ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinGetPS(bad hwnd)", WinGetPS( BAD_HWND ), PMERR_INVALID_HWND );
    check_fails( hab, "WinSubclassWindow(bad hwnd)", (ULONG)WinSubclassWindow( BAD_HWND, ClientWndProc ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinDismissDlg(bad hwnd)", WinDismissDlg( BAD_HWND, 1 ), PMERR_INVALID_HWND );
    check_fails( hab, "WinSetDlgItemText(bad hwnd)", WinSetDlgItemText( BAD_HWND, 1, "x" ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinQueryDlgItemText(bad hwnd)",
                 WinQueryDlgItemText( BAD_HWND, 1, sizeof( text ), text ), PMERR_INVALID_HWND );
    check_fails( hab, "WinSendDlgItemMsg(bad hwnd)",
                 (ULONG)WinSendDlgItemMsg( BAD_HWND, 1, WM_USER, 0, 0 ), PMERR_INVALID_HWND );
    check_fails( hab, "WinSendDlgItemMsg(no such item)",
                 (ULONG)WinSendDlgItemMsg( frame, 4242, WM_USER, 0, 0 ), PMERR_INVALID_HWND );
    check_fails( hab, "WinCreateWindow(bad parent)",
                 WinCreateWindow( BAD_HWND, WC_STATIC, "x", 0, 0, 0, 10, 10, NULLHANDLE, HWND_TOP,
                                  1, NULL, NULL ), PMERR_INVALID_HWND );
    check_fails( hab, "WinPeekMsg(bad filter)", WinPeekMsg( hab, &qmsg, BAD_HWND, 0, 0, PM_NOREMOVE ),
                 PMERR_INVALID_HWND );
    check_fails( hab, "WinGetMsg(bad filter)", WinGetMsg( hab, &qmsg, BAD_HWND, 0, 0 ), PMERR_INVALID_HWND );

    /* Parameters out of range, invalid flags and handles. */
    check_fails( hab, "WinQueryWindow(lCode 99)", WinQueryWindow( frame, 99 ), PMERR_PARAMETER_OUT_OF_RANGE );
    check_fails( hab, "WinSetWindowULong(QWL_USER, cbWindowData 0)", WinSetWindowULong( client, QWL_USER, 1 ),
                 PMERR_PARAMETER_OUT_OF_RANGE );
    check_fails( hab, "WinQueryWindowULong(below QWL_MIN)", WinQueryWindowULong( client, QWL_MIN - 1 ),
                 PMERR_PARAMETER_OUT_OF_RANGE );
    check_fails( hab, "WinInvalidateRect(fIncludeChildren 2)", WinInvalidateRect( client, NULL, 2 ),
                 PMERR_INVALID_FLAG );
    check_fails( hab, "WinBeginPaint(an HPS that is none)", (ULONG)WinBeginPaint( client, (HPS)0xBAD0, NULL ),
                 PMERR_INV_HPS );
    check_fails( hab, "WinSetDlgItemText(no such item)", WinSetDlgItemText( client, 77, "x" ),
                 PMERR_INVALID_HWND );
    {
        char text[8];
        check_fails( hab, "WinQueryDlgItemText(no such item)", WinQueryDlgItemText( client, 77, sizeof text, text ),
                     PMERR_INVALID_HWND );
    }
    check_fails( hab, "WinQueryWindowUShort(QWS_USER, cbWindowData 0)", WinQueryWindowUShort( client, QWS_USER ),
                 PMERR_PARAMETER_OUT_OF_RANGE );
    check_fails( hab, "WinPeekMsg(invalid fl)", WinPeekMsg( hab, &qmsg, NULLHANDLE, 0, 0, 0x10 ),
                 PMERR_INVALID_FLAG );
    check_fails( hab, "WinSetAccelTable(bad haccel)", WinSetAccelTable( hab, (HACCEL)0xBAD, frame ),
                 PMERR_INVALID_HACCEL );

    /* Valid calls of the same APIs record nothing. */
    check( "WinQueryWindowRect(client)", WinQueryWindowRect( client, &rcl ), TRUE );
    check( "WinQueryWindowPos(frame)", WinQueryWindowPos( frame, &swp ), TRUE );
    check( "WinQueryWindow(client, QW_PARENT)", WinQueryWindow( client, QW_PARENT ), frame );
    WinQueryWindowULong( client, QWL_STYLE );
    WinSetFocus( HWND_DESKTOP, client );
    check( "-> no error", WinGetLastError( hab ), 0 );

    /* The desktop is a valid parent: no child with that id, but no error. */
    check( "WinWindowFromID(HWND_DESKTOP, unknown id)", WinWindowFromID( HWND_DESKTOP, 4242 ), 0 );
    check( "-> no error", WinGetLastError( hab ), 0 );

    /* System values: the caret blink rate (SV_CURSORRATE, milliseconds) can
       be read and set; a rate of 0 and a handle that is no desktop fail. */
    check( "SV_CURSORRATE is positive", WinQuerySysValue( HWND_DESKTOP, SV_CURSORRATE ) > 0, TRUE );
    check( "WinSetSysValue(SV_CURSORRATE, 300)", WinSetSysValue( HWND_DESKTOP, SV_CURSORRATE, 300 ), TRUE );
    check( "-> SV_CURSORRATE reads 300", WinQuerySysValue( HWND_DESKTOP, SV_CURSORRATE ), 300 );
    check( "-> no error", WinGetLastError( hab ), 0 );
    check_fails( hab, "WinSetSysValue(SV_CURSORRATE, 0)",
                 WinSetSysValue( HWND_DESKTOP, SV_CURSORRATE, 0 ), PMERR_PARAMETER_OUT_OF_RANGE );
    check_fails( hab, "WinSetSysValue(bad hwnd)",
                 WinSetSysValue( BAD_HWND, SV_CURSORRATE, 300 ), PMERR_INVALID_HWND );
    check( "-> SV_CURSORRATE still 300", WinQuerySysValue( HWND_DESKTOP, SV_CURSORRATE ), 300 );

    /* An error survives a later successful call. */
    WinDestroyWindow( BAD_HWND );
    check( "WinSetWindowText(frame) succeeds", WinSetWindowText( frame, "PM Error Test" ), TRUE );
    check( "-> the earlier error is still there", ERRORIDERROR( WinGetLastError( hab ) ),
           PMERR_INVALID_HWND );

    /* Errors are per thread. */
    WinDestroyWindow( BAD_HWND );
    DosCreateEventSem( NULL, &thr_done, 0, FALSE );
    DosCreateThread( &tid, second_thread, 0, 0, 0x4000 );
    DosWaitEventSem( thr_done, 5000 );
    check( "DosWaitThread(second thread, which returned)", DosWaitThread( &tid, DCWW_WAIT ), NO_ERROR );
    check( "second thread: no error at start", thr_initial_err, 0 );
    check( "second thread: WinDestroyMsgQueue(main queue) fails", thr_destroy_rc, 0 );
    check( "-> PMERR_INVALID_HMQ", ERRORIDERROR( thr_destroy_err ), PMERR_INVALID_HMQ );
    check( "second thread: WinPeekMsg without a queue fails", thr_peek_rc, 0 );
    check( "-> PMERR_NO_MSG_QUEUE", ERRORIDERROR( thr_peek_err ), PMERR_NO_MSG_QUEUE );
    check( "second thread: WinCreateWindow without a queue fails", thr_create_rc, 0 );
    check( "-> PMERR_NO_MSG_QUEUE (WinCreateWindow)", ERRORIDERROR( thr_create_err ), PMERR_NO_MSG_QUEUE );
    check( "main thread keeps its own error", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INVALID_HWND );

    /* Successes the reference defines as TRUE whatever the earlier state. */
    check( "WinEnableWindow(enabled frame, TRUE)", WinEnableWindow( frame, TRUE ), TRUE );
    check( "WinSetFocus(HWND_DESKTOP) clears the focus", WinSetFocus( HWND_DESKTOP, HWND_DESKTOP ), TRUE );

    check( "WinDestroyWindow(frame)", WinDestroyWindow( frame ), TRUE );
    check( "WinDestroyMsgQueue(own queue)", WinDestroyMsgQueue( main_hmq ), TRUE );
    check( "-> no error", WinGetLastError( hab ), 0 );

    p = append_str( line, "pm_error_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinTerminate( hab );
    return 0;
}
