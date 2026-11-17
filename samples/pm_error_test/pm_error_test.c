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
#define INCL_DOSPROCESS
#define INCL_DOSSEMAPHORES
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
    WinTerminate( hab );
    DosPostEventSem( thr_done );
    /* Warpine ends the whole process on a thread's exit (DosExit with
       EXIT_THREAD or a return), so this thread idles until the program ends. */
    for( ;; ) DosSleep( 100 );
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

    /* The desktop is a valid parent: no child with that id, but no error. */
    check( "WinWindowFromID(HWND_DESKTOP, unknown id)", WinWindowFromID( HWND_DESKTOP, 4242 ), 0 );
    check( "-> no error", WinGetLastError( hab ), 0 );

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
    check( "second thread: no error at start", thr_initial_err, 0 );
    check( "second thread: WinDestroyMsgQueue(main queue) fails", thr_destroy_rc, 0 );
    check( "-> PMERR_INVALID_HMQ", ERRORIDERROR( thr_destroy_err ), PMERR_INVALID_HMQ );
    check( "second thread: WinPeekMsg without a queue fails", thr_peek_rc, 0 );
    check( "-> PMERR_NO_MSG_QUEUE", ERRORIDERROR( thr_peek_err ), PMERR_NO_MSG_QUEUE );
    check( "main thread keeps its own error", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INVALID_HWND );

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
