/*
 * pm_xproc_test.c - calls on another program's windows, under Warpine
 *
 * Warpine runs every OS/2 process as a host process of its own; on the
 * session's desktop server (ADR-0052) their window handles are unique in
 * the session, and calls on another program's window go through the
 * server to the program the window belongs to (phase B).
 *
 * The program starts itself again as a child (the argument "child" in
 * pib_pchcmd, never in argv). The child shows frame B, titled "Program B",
 * with a client of class "XProcTest" whose QWL_USER word holds 'WARK', and
 * publishes its handles and process id in named shared memory. The parent
 * (frame A) takes activation back from B (WinSetActiveWindow on its own
 * frame: B is told it lost activation), then calls on B:
 *
 *   - WinIsWindow, WinQueryWindowProcess, WinQueryWindowText,
 *     WinQueryWindowTextLength, WinQueryWindowPos, WinQueryWindowRect,
 *     WinIsWindowVisible, WinIsWindowEnabled, WinQueryClassName,
 *     WinQueryWindowULong, WinQueryWindowUShort, WinQueryWindow;
 *   - WinPostMsg, which B receives with its parameters;
 *   - WinSetWindowPos (move and size: B's client gets WM_SIZE), then
 *     WinSetActiveWindow: B becomes the active window, A loses activation,
 *     and WinQueryActiveWindow in A names B;
 *   - WinSetWindowPos(SWP_MINIMIZE) and WinShowWindow(FALSE);
 *
 * and once the child has ended, B's handle is no window any more. Each
 * check is written to stdout:
 *
 *   pm_xproc_test: <name>: ok
 *   pm_xproc_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_xproc_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include <stdio.h>

#define SHARED_NAME "\\SHAREMEM\\WARPINE_XPROC"
#define ID_POLL     1
#define POLL_MS     100
#define WAIT_POLLS  50            /* 5 s for anything asynchronous */
#define WM_XPROC    ( WM_USER + 1 )
#define USER_WORD   0x5741524BUL  /* 'WARK' */

#define LEFT_X      40
#define RIGHT_X     400
#define FRAME_Y     400
#define FRAME_CX    300
#define FRAME_CY    200
#define MOVED_X     420
#define MOVED_Y     300
#define MOVED_CX    280
#define MOVED_CY    150

typedef struct {
    ULONG child_frame;
    ULONG child_client;
    ULONG child_pid;
    LONG  child_ready;
    LONG  stop;
    LONG  child_done;
    ULONG posted_mp1;           /* WM_XPROC as B received it */
    ULONG posted_mp2;
    LONG  b_sized_cx;           /* WM_SIZE: B's client's new width */
    LONG  b_activated;          /* WM_ACTIVATE TRUE reached B */
    LONG  b_deactivated;        /* WM_ACTIVATE FALSE reached B */
} SHARED;

static HAB     hab;
static HWND    frame, client;
static SHARED *sh;
static int     is_child;
static int     step = 0;
static int     waited = 0;
static int     a_deactivated = 0;
static int     n_checks = 0, n_passed = 0;

static void write_stdout( const char *s )
{
    ULONG written;
    DosWrite( 1, (PVOID)s, strlen( s ), &written );
}

static void report( const char *what, BOOL ok, const char *got, const char *want )
{
    char line[200];
    if( ok ) {
        sprintf( line, "pm_xproc_test: %s: ok\r\n", what );
        n_passed++;
    } else {
        sprintf( line, "pm_xproc_test: %s: FAIL got %s want %s\r\n", what, got, want );
    }
    write_stdout( line );
    n_checks++;
}

static void check( const char *what, LONG got, LONG want )
{
    char g[16], w[16];
    sprintf( g, "%ld", got );
    sprintf( w, "%ld", want );
    report( what, got == want, g, w );
}

static void check_str( const char *what, const char *got, const char *want )
{
    report( what, strcmp( got, want ) == 0, got[0] ? got : "(nothing)", want );
}

/* The command line's arguments: the second string at pib_pchcmd. */
static const char *arguments( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    if( ppib->pib_pchcmd == NULL ) return "";
    return ppib->pib_pchcmd + strlen( ppib->pib_pchcmd ) + 1;
}

static ULONG own_pid( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    return ppib->pib_ulpid;
}

static APIRET start_child( void )
{
    PTIB        ptib;
    PPIB        ppib;
    char        me[CCHMAXPATH], args[CCHMAXPATH + 16];
    RESULTCODES res;
    int         n;

    DosGetInfoBlocks( &ptib, &ppib );
    if( DosQueryModuleName( ppib->pib_hmte, sizeof( me ), me ) != NO_ERROR ) return 1;
    memset( args, 0, sizeof( args ) );
    strcpy( args, me );
    n = strlen( me ) + 1;
    strcpy( args + n, "child" );
    return DosExecPgm( NULL, 0, EXEC_ASYNC, (PSZ)args, NULL, &res, (PSZ)me );
}

/* The calls answered at once by the child's window manager. */
static void query_checks( void )
{
    HWND  b = sh->child_frame, bc = sh->child_client;
    PID   pid = 0;
    TID   tid = 0;
    SWP   swp;
    RECTL rcl;
    char  buf[64];
    LONG  n;

    check( "WinIsWindow on the other program's frame", WinIsWindow( hab, b ), TRUE );
    check( "WinIsWindow on an unused handle of its slot", WinIsWindow( hab, ( b & 0xFFF00000UL ) | 0xFFFFF ), FALSE );
    check( "WinQueryWindowProcess", WinQueryWindowProcess( b, &pid, &tid ), TRUE );
    check( "  its process is the child", pid, sh->child_pid );
    check( "  which is not this process", pid != own_pid(), TRUE );
    check( "  its thread is thread 1", tid, 1 );
    memset( buf, 0, sizeof( buf ) );
    n = WinQueryWindowText( b, sizeof( buf ), buf );
    check_str( "WinQueryWindowText", buf, "Program B" );
    check( "  its length", n, 9 );
    check( "WinQueryWindowTextLength", WinQueryWindowTextLength( b ), 9 );
    memset( &swp, 0, sizeof( swp ) );
    check( "WinQueryWindowPos", WinQueryWindowPos( b, &swp ), TRUE );
    check( "  x", swp.x, RIGHT_X );
    check( "  y", swp.y, FRAME_Y );
    check( "  cx", swp.cx, FRAME_CX );
    check( "  cy", swp.cy, FRAME_CY );
    check( "  hwnd", swp.hwnd, b );
    memset( &rcl, 0xFF, sizeof( rcl ) );
    check( "WinQueryWindowRect of its client", WinQueryWindowRect( bc, &rcl ), TRUE );
    check( "  a rectangle at the origin", rcl.xLeft == 0 && rcl.yBottom == 0 && rcl.xRight > 0 && rcl.yTop > 0, TRUE );
    check( "WinIsWindowVisible", WinIsWindowVisible( b ), TRUE );
    check( "WinIsWindowEnabled", WinIsWindowEnabled( b ), TRUE );
    memset( buf, 0, sizeof( buf ) );
    WinQueryClassName( bc, sizeof( buf ), buf );
    check_str( "WinQueryClassName of its client", buf, "XProcTest" );
    check( "WinQueryWindowULong(QWL_USER)", WinQueryWindowULong( bc, QWL_USER ) == USER_WORD, TRUE );
    check( "WinQueryWindowUShort(QWS_ID)", WinQueryWindowUShort( bc, QWS_ID ), FID_CLIENT );
    check( "WinQueryWindow(QW_PARENT) of its client", WinQueryWindow( bc, QW_PARENT ) == b, TRUE );
}

/* The calls that change the child's windows. */
static void change_calls( void )
{
    HWND b = sh->child_frame;
    SWP  swp;

    check( "WinPostMsg", WinPostMsg( sh->child_client, WM_XPROC, MPFROMLONG( 42 ), MPFROMLONG( 4242 ) ), TRUE );
    check( "WinSetWindowPos (move and size)",
           WinSetWindowPos( b, HWND_TOP, MOVED_X, MOVED_Y, MOVED_CX, MOVED_CY, SWP_MOVE | SWP_SIZE ), TRUE );
    memset( &swp, 0, sizeof( swp ) );
    WinQueryWindowPos( b, &swp );
    check( "  it moved", swp.x == MOVED_X && swp.y == MOVED_Y, TRUE );
    check( "  it was sized", swp.cx == MOVED_CX && swp.cy == MOVED_CY, TRUE );
    check( "WinSetActiveWindow", WinSetActiveWindow( HWND_DESKTOP, b ), TRUE );
}

/* What reaches the child, and the activation, once it arrived. */
static BOOL arrived( void )
{
    return sh->posted_mp1 == 42 && sh->b_sized_cx != 0 && sh->b_activated && a_deactivated
        && WinQueryActiveWindow( HWND_DESKTOP ) == sh->child_frame;
}

static void arrival_checks( void )
{
    check( "the posted message reached the other program", sh->posted_mp1, 42 );
    check( "  with its second parameter", sh->posted_mp2, 4242 );
    check( "its client was told its new size", sh->b_sized_cx > 0 && sh->b_sized_cx <= MOVED_CX, TRUE );
    check( "it became the active window", sh->b_activated, 1 );
    check( "this program's frame lost activation", a_deactivated, 1 );
    check( "WinQueryActiveWindow names the other program's frame",
           WinQueryActiveWindow( HWND_DESKTOP ) == sh->child_frame, TRUE );
}

static void hide_calls( void )
{
    HWND b = sh->child_frame;
    SWP  swp;

    check( "WinSetWindowPos(SWP_MINIMIZE)", WinSetWindowPos( b, 0, 0, 0, 0, 0, SWP_MINIMIZE ), TRUE );
    memset( &swp, 0, sizeof( swp ) );
    WinQueryWindowPos( b, &swp );
    check( "  it is minimized", ( swp.fl & SWP_MINIMIZE ) != 0, TRUE );
    check( "WinShowWindow(FALSE)", WinShowWindow( b, FALSE ), TRUE );
    check( "  it is hidden", WinIsWindowVisible( b ), FALSE );
}

static void finish( void )
{
    WinStopTimer( hab, client, ID_POLL );
    WinPostMsg( client, WM_QUIT, 0, 0 );
}

/* The parent's steps, one poll at a time. */
static void parent_poll( void )
{
    waited++;
    switch( step ) {
    case 0:                                     /* the child shows B */
        if( !sh->child_ready ) {
            if( waited > WAIT_POLLS ) {
                check( "the child ran", sh->child_ready, 1 );
                finish();
            }
            return;
        }
        /* A takes activation back from B, which appeared active. */
        sh->b_deactivated = 0;
        WinSetActiveWindow( HWND_DESKTOP, frame );
        step = 1;
        waited = 0;
        return;
    case 1:                                     /* B gave activation up */
        if( !sh->b_deactivated && waited <= WAIT_POLLS ) return;
        check( "the other program's frame lost activation to this one", sh->b_deactivated, 1 );
        query_checks();
        a_deactivated = 0;
        sh->b_activated = 0;
        change_calls();
        step = 2;
        waited = 0;
        return;
    case 2:                                     /* the changes reach B */
        if( !arrived() && waited <= WAIT_POLLS ) return;
        arrival_checks();
        hide_calls();
        sh->stop = 1;
        step = 3;
        waited = 0;
        return;
    case 3:                                     /* the child ends */
        if( WinIsWindow( hab, sh->child_frame ) && waited <= WAIT_POLLS ) return;
        check( "the child ended", sh->child_done, 1 );
        check( "its frame is no window once it ended", WinIsWindow( hab, sh->child_frame ), FALSE );
        check( "WinPostMsg to it fails", WinPostMsg( sh->child_client, WM_XPROC, 0, 0 ), FALSE );
        finish();
        return;
    }
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_XPROC:
        if( is_child ) {
            sh->posted_mp1 = LONGFROMMP( mp1 );
            sh->posted_mp2 = LONGFROMMP( mp2 );
        }
        return 0;

    case WM_SIZE:
        /* Only the size the parent set: after its posted message. */
        if( is_child && sh->posted_mp1 == 42 ) sh->b_sized_cx = SHORT1FROMMP( mp2 );
        break;

    case WM_ACTIVATE:
        if( is_child && sh->child_ready ) {
            if( SHORT1FROMMP( mp1 ) ) sh->b_activated = 1; else sh->b_deactivated = 1;
        }
        if( !is_child && !SHORT1FROMMP( mp1 ) ) a_deactivated = 1;
        break;

    case WM_TIMER:
        if( SHORT1FROMMP( mp1 ) != ID_POLL ) break;
        if( !is_child ) {
            parent_poll();
        } else if( sh->stop ) {
            sh->child_done = 1;
            WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        }
        return 0;

    case WM_PAINT: {
        RECTL rc;
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinFillRect( hps, &rc, is_child ? CLR_WHITE : CLR_PALEGRAY );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_MINBUTTON;
    char  line[64];
    PVOID mem;

    is_child = strcmp( arguments(), "child" ) == 0;
    if( is_child ) {
        if( DosGetNamedSharedMem( &mem, (PSZ)SHARED_NAME, PAG_READ | PAG_WRITE ) != NO_ERROR ) return 2;
    } else {
        if( DosAllocSharedMem( &mem, (PSZ)SHARED_NAME, sizeof( SHARED ),
                               PAG_COMMIT | PAG_READ | PAG_WRITE ) != NO_ERROR ) return 2;
        memset( mem, 0, sizeof( SHARED ) );
    }
    sh = (SHARED *)mem;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "XProcTest", ClientWndProc, CS_SIZEREDRAW, sizeof( ULONG ) );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "XProcTest",
                                is_child ? "Program B" : "Program A", 0, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, is_child ? RIGHT_X : LEFT_X, FRAME_Y, FRAME_CX, FRAME_CY,
                     SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    WinStartTimer( hab, client, ID_POLL, POLL_MS );
    if( is_child ) {
        WinSetWindowULong( client, QWL_USER, USER_WORD );
        sh->child_frame = frame;
        sh->child_client = client;
        sh->child_pid = own_pid();
        sh->child_ready = 1;
    } else if( start_child() != NO_ERROR ) {
        write_stdout( "pm_xproc_test: cannot start the child\r\n" );
    }

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    if( !is_child ) {
        sprintf( line, "pm_xproc_test: done %d/%d\r\n", n_passed, n_checks );
        write_stdout( line );
    }
    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
