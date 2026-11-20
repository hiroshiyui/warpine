/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_xshell_test.c - the session's shell services across programs, under
 * Warpine (ADR-0056)
 *
 * The program starts itself again as a child (the argument "child" in
 * pib_pchcmd, never in argv). Both show an FCF_TASKLIST frame: "Shell A"
 * (the parent) and "Shell B" (the child). The child adds the system atom
 * "Warpine Shell Atom" and publishes its handles in named shared memory.
 * The parent then checks:
 *
 *   - the switch list is the session's: WinQuerySwitchList lists both
 *     programs' entries, WinQuerySwitchHandle / WinQuerySwitchEntry find
 *     B's (its title, process), an entry the parent adds is seen by the
 *     child, B's entry cannot be changed by the parent;
 *   - the system atom table is the session's: WinFindAtom finds B's atom
 *     (without regard to case), WinQueryAtomName, WinQueryAtomUsage;
 *   - DDE between the programs: WinDdeInitiate reaches B's client, which
 *     answers with WinDdeRespond before WinDdeInitiate returns; a
 *     WM_DDE_REQUEST in shared memory gets B's WM_DDE_DATA back;
 *   - WinSwitchToProgram(B's entry) makes B the active window;
 *   - the window list: with A active again, the GUI script types Ctrl+Esc,
 *     Down, Enter: the second entry ("Shell B") becomes active;
 *
 * and once the child has ended its entries are gone from the list. Each
 * check is written to stdout:
 *
 *   pm_xshell_test: <name>: ok
 *   pm_xshell_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_xshell_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_WINSWITCHLIST
#define INCL_WINATOM
#define INCL_WINDDE
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include <stdio.h>

#define SHARED_NAME "\\SHAREMEM\\WARPINE_XSHELL"
#define ID_POLL     1
#define POLL_MS     100
#define WAIT_POLLS  50            /* 5 s for anything asynchronous */
#define LIST_POLLS  150           /* the window list waits for the script */
#define ATOM_NAME   "Warpine Shell Atom"
#define APP_NAME    "WarpineShell"
#define TOPIC       "Phase C"
#define ITEM        "Greeting"
#define GREETING    "Hello from B"

typedef struct {
    ULONG child_frame;
    ULONG child_client;
    ULONG child_pid;
    LONG  child_ready;
    ULONG child_atom;
    LONG  extra_seen;             /* the child saw the parent's entry */
    LONG  stop;
    LONG  child_done;
    LONG  dde_names_ok;           /* WM_DDE_INITIATE's DDEINIT at B */
    LONG  dde_request_ok;         /* WM_DDE_REQUEST's item at B */
} SHARED;

static HAB     hab;
static HWND    frame, client;
static SHARED *sh;
static int     is_child;
static int     step = 0;
static int     waited = 0;
static HWND    dde_server = NULLHANDLE;
static char    dde_data[32] = "";
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
        sprintf( line, "pm_xshell_test: %s: ok\r\n", what );
        n_passed++;
    } else {
        sprintf( line, "pm_xshell_test: %s: FAIL got %s want %s\r\n", what, got, want );
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

/* A DDESTRUCT in giveable shared memory. */
static PDDESTRUCT make_dde( const char *item, const void *data, ULONG cb )
{
    PDDESTRUCT p;
    ULONG      len = strlen( item ) + 1;
    if( DosAllocSharedMem( (PPVOID)&p, NULL, sizeof( DDESTRUCT ) + len + cb,
                           PAG_COMMIT | PAG_READ | PAG_WRITE | OBJ_GIVEABLE ) != NO_ERROR )
        return NULL;
    p->cbData = cb;
    p->fsStatus = 0;
    p->usFormat = DDEFMT_TEXT;
    p->offszItemName = sizeof( DDESTRUCT );
    p->offabData = (USHORT)( sizeof( DDESTRUCT ) + len );
    memcpy( DDES_PSZITEMNAME( p ), item, len );
    if( cb ) memcpy( DDES_PABDATA( p ), data, cb );
    return p;
}

/* Whether the session's switch list holds an entry titled `title`; its
   handle in *found. */
static BOOL find_entry( const char *title, HSWITCH *found )
{
    static char buf[sizeof( SWBLOCK ) + 32 * sizeof( SWENTRY )];
    PSWBLOCK    blk = (PSWBLOCK)buf;
    ULONG       i, n;

    memset( buf, 0, sizeof( buf ) );
    n = WinQuerySwitchList( hab, blk, sizeof( buf ) );
    for( i = 0; i < n; i++ ) {
        if( strcmp( blk->aswentry[i].swctl.szSwtitle, title ) == 0 ) {
            if( found ) *found = blk->aswentry[i].hswitch;
            return TRUE;
        }
    }
    return FALSE;
}

static void switch_checks( void )
{
    HSWITCH a = NULLHANDLE, b = NULLHANDLE, extra;
    SWCNTRL ctl;

    check( "WinQuerySwitchList lists this program's frame", find_entry( "Shell A", &a ), TRUE );
    check( "  and the other program's", find_entry( "Shell B", &b ), TRUE );
    check( "WinQuerySwitchHandle of the other program's frame", WinQuerySwitchHandle( sh->child_frame, 0 ), b );
    check( "WinQuerySwitchHandle(NULLHANDLE, its process)", WinQuerySwitchHandle( NULLHANDLE, sh->child_pid ), b );
    memset( &ctl, 0, sizeof( ctl ) );
    check( "WinQuerySwitchEntry of its entry", WinQuerySwitchEntry( b, &ctl ), 0 );
    check_str( "  its title", ctl.szSwtitle, "Shell B" );
    check( "  its process", ctl.idProcess, sh->child_pid );
    check( "  its frame", ctl.hwnd == sh->child_frame, TRUE );
    strcpy( ctl.szSwtitle, "Taken over" );
    check( "WinChangeSwitchEntry of another program's entry fails", WinChangeSwitchEntry( b, &ctl ) != 0, TRUE );

    memset( &ctl, 0, sizeof( ctl ) );
    ctl.hwnd = frame;
    ctl.uchVisibility = SWL_INVISIBLE;
    ctl.fbJump = SWL_JUMPABLE;
    strcpy( ctl.szSwtitle, "Extra A" );
    extra = WinAddSwitchEntry( &ctl );
    check( "WinAddSwitchEntry", extra != NULLHANDLE, TRUE );
}

static void atom_checks( void )
{
    HATOMTBL sys = WinQuerySystemAtomTable();
    char     buf[64];

    check( "WinFindAtom finds the other program's system atom",
           WinFindAtom( sys, "WARPINE SHELL ATOM" ), sh->child_atom );
    memset( buf, 0, sizeof( buf ) );
    WinQueryAtomName( sys, sh->child_atom, buf, sizeof( buf ) );
    check_str( "WinQueryAtomName", buf, ATOM_NAME );
    check( "WinAddAtom counts one more use", WinAddAtom( sys, ATOM_NAME ), sh->child_atom );
    check( "WinQueryAtomUsage", WinQueryAtomUsage( sys, sh->child_atom ), 2 );
    check( "WinDeleteAtom", WinDeleteAtom( sys, sh->child_atom ), 0 );
}

static void dde_checks( void )
{
    PDDESTRUCT p;

    check( "WinDdeInitiate", WinDdeInitiate( client, APP_NAME, TOPIC, NULL ), TRUE );
    check( "  the other program answered before it returned", dde_server == sh->child_client, TRUE );
    check( "  its WM_DDE_INITIATE named the application and topic", sh->dde_names_ok, 1 );
    p = make_dde( ITEM, NULL, 0 );
    check( "WinDdePostMsg(WM_DDE_REQUEST) to the other program",
           WinDdePostMsg( dde_server, client, WM_DDE_REQUEST, p, 0 ), TRUE );
}

static void finish( void )
{
    WinStopTimer( hab, client, ID_POLL );
    WinPostMsg( client, WM_QUIT, 0, 0 );
}

/* The parent's steps, one poll at a time. */
static void parent_poll( void )
{
    HSWITCH b = NULLHANDLE;

    waited++;
    switch( step ) {
    case 0:                                     /* the child is up */
        if( !sh->child_ready ) {
            if( waited > WAIT_POLLS ) {
                check( "the child ran", sh->child_ready, 1 );
                finish();
            }
            return;
        }
        switch_checks();
        atom_checks();
        dde_checks();
        step = 1;
        waited = 0;
        return;
    case 1:                                     /* the DDE data and our entry reach us */
        if( ( !dde_data[0] || !sh->extra_seen ) && waited <= WAIT_POLLS ) return;
        check_str( "WM_DDE_DATA from the other program", dde_data, GREETING );
        check( "  which saw the request's item", sh->dde_request_ok, 1 );
        check( "the other program sees this program's entry", sh->extra_seen, 1 );
        find_entry( "Shell B", &b );
        check( "WinSwitchToProgram of the other program's entry", WinSwitchToProgram( b ), 0 );
        step = 2;
        waited = 0;
        return;
    case 2:                                     /* B active; then A again */
        if( WinQueryActiveWindow( HWND_DESKTOP ) != sh->child_frame && waited <= WAIT_POLLS ) return;
        check( "  it became the active window", WinQueryActiveWindow( HWND_DESKTOP ) == sh->child_frame, TRUE );
        WinSetActiveWindow( HWND_DESKTOP, frame );
        step = 3;
        waited = 0;
        return;
    case 3:                                     /* Ctrl+Esc, Down, Enter */
        if( WinQueryActiveWindow( HWND_DESKTOP ) != sh->child_frame && waited <= LIST_POLLS ) return;
        check( "the window list switched to the second entry",
               WinQueryActiveWindow( HWND_DESKTOP ) == sh->child_frame, TRUE );
        sh->stop = 1;
        step = 4;
        waited = 0;
        return;
    case 4:                                     /* the child ends */
        if( !sh->child_done && waited <= WAIT_POLLS ) return;
        DosSleep( 500 );
        check( "the child ended", sh->child_done, 1 );
        check( "its entries left the switch list", find_entry( "Shell B", NULL ), FALSE );
        check( "this program's are still there", find_entry( "Shell A", NULL ), TRUE );
        finish();
        return;
    }
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    PDDESTRUCT p = (PDDESTRUCT)mp2;

    switch( msg ) {
    case WM_DDE_INITIATE: {
        PDDEINIT init = (PDDEINIT)mp2;
        if( is_child && strcmp( init->pszAppName, APP_NAME ) == 0 ) {
            sh->dde_names_ok = strcmp( init->pszTopic, TOPIC ) == 0;
            WinDdeRespond( HWNDFROMMP( mp1 ), hwnd, APP_NAME, TOPIC, NULL );
        }
        DosFreeMem( init );
        return 0;
    }
    case WM_DDE_INITIATEACK: {
        PDDEINIT init = (PDDEINIT)mp2;
        if( !is_child && strcmp( init->pszAppName, APP_NAME ) == 0 ) dde_server = HWNDFROMMP( mp1 );
        DosFreeMem( init );
        return (MRESULT)TRUE;
    }
    case WM_DDE_REQUEST:
        if( is_child ) {
            sh->dde_request_ok = strcmp( DDES_PSZITEMNAME( p ), ITEM ) == 0;
            DosFreeMem( p );
            WinDdePostMsg( HWNDFROMMP( mp1 ), hwnd, WM_DDE_DATA,
                           make_dde( ITEM, GREETING, sizeof( GREETING ) ), 0 );
        }
        return 0;
    case WM_DDE_DATA:
        if( !is_child ) {
            if( p->cbData <= sizeof( dde_data ) ) memcpy( dde_data, DDES_PABDATA( p ), p->cbData );
            DosFreeMem( p );
        }
        return 0;

    case WM_TIMER:
        if( SHORT1FROMMP( mp1 ) != ID_POLL ) break;
        if( !is_child ) {
            parent_poll();
        } else {
            if( !sh->extra_seen && find_entry( "Extra A", NULL ) ) sh->extra_seen = 1;
            if( sh->stop ) {
                sh->child_done = 1;
                WinPostMsg( hwnd, WM_QUIT, 0, 0 );
            }
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
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_TASKLIST;
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
    WinRegisterClass( hab, "XShellTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "XShellTest",
                                is_child ? "Shell B" : "Shell A", 0, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, is_child ? 400 : 40, 400, 300, 200,
                     SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    WinStartTimer( hab, client, ID_POLL, POLL_MS );
    if( is_child ) {
        sh->child_atom = WinAddAtom( WinQuerySystemAtomTable(), ATOM_NAME );
        sh->child_frame = frame;
        sh->child_client = client;
        sh->child_pid = own_pid();
        sh->child_ready = 1;
    } else if( start_child() != NO_ERROR ) {
        write_stdout( "pm_xshell_test: cannot start the child\r\n" );
    }

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    if( !is_child ) {
        sprintf( line, "pm_xshell_test: done %d/%d\r\n", n_passed, n_checks );
        write_stdout( line );
    }
    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
