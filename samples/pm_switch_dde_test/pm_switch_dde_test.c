/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_switch_dde_test.c - the switch list, atom tables, DDE, WinStartApp /
 * WinTerminateApp and the MSG.DLL helpers under Warpine
 *
 * Switch list:
 *   - a frame created with FCF_TASKLIST has an entry (its title, its
 *     process), found by WinQuerySwitchHandle from the frame or client
 *   - WinAddSwitchEntry / WinCreateSwitchEntry / WinChangeSwitchEntry /
 *     WinQuerySwitchEntry / WinRemoveSwitchEntry, WinQuerySwitchList
 *     (count, then the SWBLOCK), WinSwitchToProgram (activates the frame)
 *   - the entry goes when its frame is destroyed
 *
 * Atoms:
 *   - string atoms (case-insensitive, usage counts, names, lengths),
 *     integer atoms (MAKEINTATOM, "#n"), invalid atoms and tables
 *   - the system atom table is separate from a private one
 *
 * DDE (two frames of this process):
 *   - WinDdeInitiate reaches the server frame, which answers with
 *     WinDdeRespond (WM_DDE_INITIATEACK arrives before WinDdeInitiate
 *     returns)
 *   - WM_DDE_REQUEST -> WM_DDE_DATA, WM_DDE_EXECUTE -> WM_DDE_ACK,
 *     WM_DDE_TERMINATE both ways, DDESTRUCTs in giveable shared memory
 *     freed by their receivers; WinDdePostMsg rejects a non-DDE message
 *
 * Applications:
 *   - WinStartApp runs this program again ("child alpha", an added
 *     environment variable); WM_APPTERMINATENOTIFY brings its return code
 *   - a second copy that sleeps is ended by WinTerminateApp and still
 *     notifies; bad handles fail with PMERR_INVALID_HAPP
 *
 * MSG.DLL:
 *   - DosInsertMessage (%1, %2, truncation, too many strings)
 *   - DosPutMessage writes one of the check lines
 *   - DosGetMessage fills the buffer (argument order of MSG.6)
 *   - DosQueryMessageCP (DOS32IQUERYMESSAGECP, MSG.8) for a file that
 *     exists (C:\SWDDE.MSG, created empty) and one that does not
 *
 * Each check is written to stdout:
 *
 *   pm_switch_dde_test: <name>: ok
 *   pm_switch_dde_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_switch_dde_test: done <passed>/<total>". The copies
 * started by WinStartApp (told apart by "child" in the command line read
 * from pib_pchcmd) write nothing and end with code 7 (arguments and
 * environment as expected), 8 (not) or 9 (the sleeper, if not ended).
 */
#define INCL_WIN
#define INCL_WINSWITCHLIST
#define INCL_WINPROGRAMLIST
#define INCL_WINATOM
#define INCL_WINDDE
#define INCL_WINERRORS
#define INCL_SHLERRORS
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define PREFIX     "pm_switch_dde_test: "
#define TITLE      "Switch DDE Test"
#define APP_NAME   "WarpineSrv"
#define TOPIC      "System"
#define ITEM       "Time"
#define EXEC_CMD   "[run]"
#define CHILD_RC   7
#define MSG_FILE   "C:\\SWDDE.MSG"
#define TIMEOUT_ID 1
#define TIMEOUT_MS 20000

/* The 32-bit DosQueryMessageCP entry point in os2386.lib (MSG.8). */
APIRET APIENTRY DOS32IQUERYMESSAGECP( PCHAR pb, ULONG cb, PCSZ pszFile, PULONG pcbBuf );

static int  n_checks = 0;
static int  n_passed = 0;
static HAB  hab;
static HWND client, server_client;
static HWND dde_server = NULLHANDLE;
static HAPP happ_child = NULLHANDLE, happ_sleeper = NULLHANDLE;
static char exe_path[CCHMAXPATH];

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
    char *p = append_str( line, PREFIX );
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

static void check_error( const char *what, USHORT code )
{
    check( what, ERRORIDERROR( WinGetLastError( hab ) ), code );
}

static BOOL same( const void *a, const void *b, ULONG n )
{
    return memcmp( a, b, n ) == 0;
}

static void finish( void )
{
    char line[64], *p;
    p = append_str( line, PREFIX "done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );
}

/* ---------- the child copies ---------- */

/* The argument string of this process's command line: the second string
 * at pib_pchcmd (the C runtime's argv is not used). */
static const char *command_args( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    if( ppib->pib_pchcmd == NULL ) return "";
    return ppib->pib_pchcmd + strlen( ppib->pib_pchcmd ) + 1;
}

/* Whether args starts with word w (followed by a blank or the end). */
static BOOL starts_with_word( const char *args, const char *w )
{
    ULONG n = strlen( w );
    while( *args == ' ' ) args++;
    return strncmp( args, w, n ) == 0 && ( args[n] == ' ' || args[n] == '\0' );
}

static int child_main( const char *args )
{
    PSZ value;
    if( strcmp( args, "child sleep" ) == 0 ) {
        DosSleep( 30000 );
        return 9;
    }
    if( strcmp( args, "child alpha" ) != 0 ) return 8;
    if( DosScanEnv( "SWDDE", &value ) != NO_ERROR || strcmp( value, "yes" ) != 0 ) return 8;
    return CHILD_RC;
}

/* ---------- DDE helpers ---------- */

static PDDESTRUCT make_dde( const char *item, USHORT fmt, const void *data, ULONG cb, USHORT status )
{
    PDDESTRUCT p;
    ULONG      len = strlen( item ) + 1;
    if( DosAllocSharedMem( (PPVOID)&p, NULL, sizeof( DDESTRUCT ) + len + cb,
                           PAG_COMMIT | PAG_READ | PAG_WRITE | OBJ_GIVEABLE ) != NO_ERROR )
        return NULL;
    p->cbData = cb;
    p->fsStatus = status;
    p->usFormat = fmt;
    p->offszItemName = sizeof( DDESTRUCT );
    p->offabData = (USHORT)( sizeof( DDESTRUCT ) + len );
    memcpy( DDES_PSZITEMNAME( p ), item, len );
    if( cb ) memcpy( DDES_PABDATA( p ), data, cb );
    return p;
}

static void start_child( void )
{
    PROGDETAILS pd;
    memset( &pd, 0, sizeof( pd ) );
    pd.Length = sizeof( pd );
    pd.progt.progc = PROG_DEFAULT;
    pd.progt.fbVisible = SHE_VISIBLE;
    pd.pszExecutable = exe_path;
    pd.pszParameters = "ignored";
    pd.pszEnvironment = "SWDDE=yes\0";
    happ_child = WinStartApp( client, &pd, "child alpha", NULL, SAF_STARTCHILDAPP );
    check( "WinStartApp starts this program again", happ_child != NULLHANDLE, TRUE );
}

static void start_sleeper( void )
{
    PROGDETAILS pd;
    memset( &pd, 0, sizeof( pd ) );
    pd.Length = sizeof( pd );
    pd.progt.progc = PROG_DEFAULT;
    pd.pszExecutable = exe_path;
    pd.pszParameters = "child sleep";
    happ_sleeper = WinStartApp( client, &pd, NULL, NULL, SAF_INSTALLEDCMDLINE );
    check( "WinStartApp starts a sleeping copy", happ_sleeper != NULLHANDLE, TRUE );
    check( "WinTerminateApp ends it", WinTerminateApp( happ_sleeper ), TRUE );
}

/* ---------- window procedures ---------- */

static MRESULT EXPENTRY ServerWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    PDDESTRUCT p = (PDDESTRUCT)mp2;
    switch( msg ) {
    case WM_DDE_INITIATE: {
        PDDEINIT init = (PDDEINIT)mp2;
        if( strcmp( init->pszAppName, APP_NAME ) == 0 && strcmp( init->pszTopic, TOPIC ) == 0 ) {
            check( "server: WM_DDE_INITIATE names the application and topic", TRUE, TRUE );
            check( "server: the context is passed", init->offConvContext != 0
                   && DDEI_PCONVCONTEXT( init )->fsContext == DDECTXT_CASESENSITIVE, TRUE );
            WinDdeRespond( HWNDFROMMP( mp1 ), hwnd, APP_NAME, TOPIC, NULL );
        }
        DosFreeMem( init );
        return 0;
    }
    case WM_DDE_REQUEST:
        check( "server: WM_DDE_REQUEST for the item", strcmp( DDES_PSZITEMNAME( p ), ITEM ), 0 );
        DosFreeMem( p );
        p = make_dde( ITEM, DDEFMT_TEXT, "42", 3, DDE_FRESPONSE );
        check( "server: WinDdePostMsg(WM_DDE_DATA)",
               WinDdePostMsg( HWNDFROMMP( mp1 ), hwnd, WM_DDE_DATA, p, DDEPM_RETRY ), TRUE );
        return 0;
    case WM_DDE_EXECUTE:
        check( "server: WM_DDE_EXECUTE carries the command",
               p->cbData == sizeof( EXEC_CMD ) && same( DDES_PABDATA( p ), EXEC_CMD, sizeof( EXEC_CMD ) ), TRUE );
        DosFreeMem( p );
        WinDdePostMsg( HWNDFROMMP( mp1 ), hwnd, WM_DDE_ACK, make_dde( "", 0, NULL, 0, DDE_FACK ), 0 );
        return 0;
    case WM_DDE_TERMINATE:
        if( p ) DosFreeMem( p );
        WinDdePostMsg( HWNDFROMMP( mp1 ), hwnd, WM_DDE_TERMINATE, make_dde( "", 0, NULL, 0, 0 ), 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    PDDESTRUCT p = (PDDESTRUCT)mp2;
    switch( msg ) {
    case WM_DDE_INITIATE:
        /* Our own initiation reaches our frame too: not a server. */
        DosFreeMem( mp2 );
        return 0;
    case WM_DDE_INITIATEACK: {
        PDDEINIT init = (PDDEINIT)mp2;
        dde_server = HWNDFROMMP( mp1 );
        check( "client: WM_DDE_INITIATEACK names the application",
               strcmp( init->pszAppName, APP_NAME ) == 0 && strcmp( init->pszTopic, TOPIC ) == 0, TRUE );
        DosFreeMem( init );
        return (MRESULT)TRUE;
    }
    case WM_DDE_DATA:
        check( "client: WM_DDE_DATA from the server", HWNDFROMMP( mp1 ) == dde_server, TRUE );
        check( "client: the item, format, status and data",
               strcmp( DDES_PSZITEMNAME( p ), ITEM ) == 0 && p->usFormat == DDEFMT_TEXT
               && ( p->fsStatus & DDE_FRESPONSE ) && p->cbData == 3
               && same( DDES_PABDATA( p ), "42", 3 ), TRUE );
        DosFreeMem( p );
        WinDdePostMsg( dde_server, hwnd, WM_DDE_EXECUTE,
                       make_dde( "", DDEFMT_TEXT, EXEC_CMD, sizeof( EXEC_CMD ), 0 ), DDEPM_RETRY );
        return 0;
    case WM_DDE_ACK:
        check( "client: WM_DDE_ACK is positive", ( p->fsStatus & DDE_FACK ) != 0, TRUE );
        DosFreeMem( p );
        WinDdePostMsg( dde_server, hwnd, WM_DDE_TERMINATE, make_dde( "", 0, NULL, 0, 0 ), 0 );
        return 0;
    case WM_DDE_TERMINATE:
        check( "client: the server terminates too", HWNDFROMMP( mp1 ) == dde_server, TRUE );
        if( p ) DosFreeMem( p );
        start_child();
        return 0;
    case WM_APPTERMINATENOTIFY:
        if( (HAPP)mp1 == happ_child && happ_child != NULLHANDLE ) {
            check( "WM_APPTERMINATENOTIFY: the child's return code", LONGFROMMP( mp2 ), CHILD_RC );
            happ_child = NULLHANDLE;
            check( "WinTerminateApp of an ended application fails",
                   WinTerminateApp( (HAPP)mp1 ), FALSE );
            check_error( "-> PMERR_INVALID_HAPP", PMERR_INVALID_HAPP );
            start_sleeper();
        } else if( (HAPP)mp1 == happ_sleeper && happ_sleeper != NULLHANDLE ) {
            check( "WM_APPTERMINATENOTIFY for the ended copy", LONGFROMMP( mp2 ) != 9, TRUE );
            happ_sleeper = NULLHANDLE;
            WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        }
        return 0;
    case WM_TIMER:
        if( SHORT1FROMMP( mp1 ) == TIMEOUT_ID ) {
            check( "the conversation and applications end in time", FALSE, TRUE );
            WinPostMsg( hwnd, WM_QUIT, 0, 0 );
            return 0;
        }
        break;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

/* ---------- synchronous checks ---------- */

static void switch_checks( HWND frame )
{
    HSWITCH  hsw, extra;
    SWCNTRL  ctl;
    PSWBLOCK blk;
    char     buf[sizeof( SWBLOCK ) + 3 * sizeof( SWENTRY )];
    PTIB     ptib;
    PPIB     ppib;

    DosGetInfoBlocks( &ptib, &ppib );
    check( "WinQuerySwitchList counts the FCF_TASKLIST entry", WinQuerySwitchList( hab, NULL, 0 ), 1 );
    hsw = WinQuerySwitchHandle( frame, 0 );
    check( "WinQuerySwitchHandle(frame)", hsw != NULLHANDLE, TRUE );
    check( "WinQuerySwitchHandle(client) finds the frame's entry", WinQuerySwitchHandle( client, 0 ), hsw );
    check( "WinQuerySwitchHandle(NULLHANDLE, pid)", WinQuerySwitchHandle( NULLHANDLE, ppib->pib_ulpid ), hsw );
    memset( &ctl, 0, sizeof( ctl ) );
    check( "WinQuerySwitchEntry", WinQuerySwitchEntry( hsw, &ctl ), 0 );
    check( "-> the frame", ctl.hwnd == frame, TRUE );
    check( "-> the title", strcmp( ctl.szSwtitle, TITLE ), 0 );
    check( "-> this process", ctl.idProcess, ppib->pib_ulpid );

    memset( &ctl, 0, sizeof( ctl ) );
    ctl.uchVisibility = SWL_VISIBLE;
    ctl.fbJump = SWL_JUMPABLE;
    strcpy( ctl.szSwtitle, "Extra" );
    extra = WinAddSwitchEntry( &ctl );
    check( "WinAddSwitchEntry", extra != NULLHANDLE && extra != hsw, TRUE );
    check( "-> two entries", WinQuerySwitchList( hab, NULL, 0 ), 2 );
    strcpy( ctl.szSwtitle, "Changed" );
    check( "WinChangeSwitchEntry", WinChangeSwitchEntry( extra, &ctl ), 0 );
    memset( buf, 0, sizeof( buf ) );
    blk = (PSWBLOCK)buf;
    check( "WinQuerySwitchList fills the SWBLOCK", WinQuerySwitchList( hab, blk, sizeof( buf ) ), 2 );
    check( "-> cswentry", blk->cswentry, 2 );
    check( "-> the changed entry",
           blk->aswentry[1].hswitch == extra && strcmp( blk->aswentry[1].swctl.szSwtitle, "Changed" ) == 0, TRUE );
    check( "a short SWBLOCK gets what fits",
           WinQuerySwitchList( hab, blk, sizeof( SWBLOCK ) ), 1 );
    check( "WinRemoveSwitchEntry", WinRemoveSwitchEntry( extra ), 0 );
    check( "WinRemoveSwitchEntry again fails", WinRemoveSwitchEntry( extra ) != 0, TRUE );
    check( "WinQuerySwitchEntry of a removed entry fails", WinQuerySwitchEntry( extra, &ctl ) != 0, TRUE );
    extra = WinCreateSwitchEntry( hab, &ctl );
    check( "WinCreateSwitchEntry", extra != NULLHANDLE, TRUE );
    WinRemoveSwitchEntry( extra );
    check( "-> one entry left", WinQuerySwitchList( hab, NULL, 0 ), 1 );
    check( "WinSwitchToProgram", WinSwitchToProgram( hsw ), 0 );
    check( "-> the frame is active", WinQueryActiveWindow( HWND_DESKTOP ) == frame, TRUE );
    check( "WinSwitchToProgram of a bad handle fails", WinSwitchToProgram( (HSWITCH)0x7777 ) != 0, TRUE );
}

static void atom_checks( void )
{
    HATOMTBL sys, tbl;
    ATOM     a;
    char     buf[16];

    sys = WinQuerySystemAtomTable();
    tbl = WinCreateAtomTable( 0, 0 );
    check( "WinQuerySystemAtomTable", sys != NULLHANDLE, TRUE );
    check( "WinCreateAtomTable", tbl != NULLHANDLE && tbl != sys, TRUE );
    a = WinAddAtom( tbl, "Hello" );
    check( "WinAddAtom gives a string atom", a >= 0xC000, TRUE );
    check( "the same name in another case is the same atom", WinAddAtom( tbl, "HELLO" ), a );
    check( "WinQueryAtomUsage counts both", WinQueryAtomUsage( tbl, a ), 2 );
    check( "WinFindAtom ignores case", WinFindAtom( tbl, "hello" ), a );
    check( "WinQueryAtomLength", WinQueryAtomLength( tbl, a ), 5 );
    memset( buf, 0x55, sizeof( buf ) );
    check( "WinQueryAtomName", WinQueryAtomName( tbl, a, buf, sizeof( buf ) ), 5 );
    check( "-> the first spelling", strcmp( buf, "Hello" ), 0 );
    check( "WinQueryAtomName cuts to the buffer", WinQueryAtomName( tbl, a, buf, 3 ), 2 );
    check( "-> NUL-terminated", strcmp( buf, "He" ), 0 );
    check( "the system table does not have it", WinFindAtom( sys, "Hello" ), 0 );
    check_error( "-> PMERR_ATOM_NAME_NOT_FOUND", PMERR_ATOM_NAME_NOT_FOUND );
    check( "WinDeleteAtom", WinDeleteAtom( tbl, a ), 0 );
    check( "-> one use left", WinQueryAtomUsage( tbl, a ), 1 );
    WinDeleteAtom( tbl, a );
    check( "-> gone after the last", WinFindAtom( tbl, "Hello" ), 0 );
    check( "WinDeleteAtom of a deleted atom returns it", WinDeleteAtom( tbl, a ), a );
    check_error( "-> PMERR_INVALID_ATOM", PMERR_INVALID_ATOM );

    check( "MAKEINTATOM(123) is atom 123", WinAddAtom( tbl, MAKEINTATOM( 123 ) ), 123 );
    check( "\"#456\" is atom 456", WinAddAtom( tbl, "#456" ), 456 );
    check( "WinQueryAtomName of an integer atom", WinQueryAtomName( tbl, 123, buf, sizeof( buf ) ), 4 );
    check( "-> \"#123\"", strcmp( buf, "#123" ), 0 );
    check( "MAKEINTATOM(0) is invalid", WinAddAtom( tbl, MAKEINTATOM( 0 ) ), 0 );
    check_error( "-> PMERR_INVALID_INTEGER_ATOM", PMERR_INVALID_INTEGER_ATOM );
    check( "an empty name is invalid", WinAddAtom( tbl, "" ), 0 );
    check_error( "-> PMERR_INVALID_ATOM_NAME", PMERR_INVALID_ATOM_NAME );

    a = WinAddAtom( sys, "Warpine.Test" );
    check( "WinAddAtom in the system table", a >= 0xC000, TRUE );
    check( "WinDestroyAtomTable", WinDestroyAtomTable( tbl ), NULLHANDLE );
    check( "WinDestroyAtomTable again returns the handle", WinDestroyAtomTable( tbl ), tbl );
    check_error( "-> PMERR_INVALID_HATOMTBL", PMERR_INVALID_HATOMTBL );
    check( "WinAddAtom in a destroyed table fails", WinAddAtom( tbl, "x" ), 0 );
    check( "the system table keeps its atom", WinFindAtom( sys, "warpine.test" ), a );
}

static void msg_checks( void )
{
    PCHAR  table[2];
    char   buf[64], line[96], *p;
    ULONG  cb, rc, action;
    HFILE  hf;
    APIRET r;
    static const char msg[] = "Hello %1, %2!";

    table[0] = "OS/2";
    table[1] = "Warpine";
    memset( buf, 0x55, sizeof( buf ) );
    r = DosInsertMessage( table, 2, msg, sizeof( msg ) - 1, buf, sizeof( buf ), &cb );
    check( "DosInsertMessage", r, NO_ERROR );
    check( "-> the length", cb, 20 );
    check( "-> the text", same( buf, "Hello OS/2, Warpine!", 20 ), TRUE );
    r = DosInsertMessage( table, 2, msg, sizeof( msg ) - 1, buf, 8, &cb );
    check( "DosInsertMessage into a short buffer", r, ERROR_MR_MSG_TOO_LONG );
    r = DosInsertMessage( table, 10, msg, sizeof( msg ) - 1, buf, sizeof( buf ), &cb );
    check( "DosInsertMessage with ten strings", r, ERROR_MR_INV_IVCOUNT );

    p = append_str( line, PREFIX "DosPutMessage writes this line: ok\r\n" );
    rc = DosPutMessage( (HFILE)1, p - line, line );
    n_checks++;
    if( rc == NO_ERROR ) {
        n_passed++;
    } else {
        write_stdout( PREFIX "DosPutMessage writes this line: FAIL\r\n" );
    }

    cb = 0;
    memset( buf, 0, sizeof( buf ) );
    DosGetMessage( table, 0, buf, sizeof( buf ), 1, "OSO001.MSG", &cb );
    check( "DosGetMessage fills the buffer", cb > 0 && cb <= sizeof( buf ) && buf[0] != 0, TRUE );

    /* A file that exists (its contents are not read). */
    if( DosOpen( MSG_FILE, &hf, &action, 0, FILE_NORMAL, OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_OPEN_IF_EXISTS,
                 OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL ) == NO_ERROR )
        DosClose( hf );
    cb = 0;
    r = DOS32IQUERYMESSAGECP( buf, sizeof( buf ), MSG_FILE, &cb );
    check( "DosQueryMessageCP of an existing file", r, NO_ERROR );
    check( "-> at least one codepage", cb >= 2 * sizeof( USHORT ) && *(PUSHORT)buf >= 1, TRUE );
    r = DOS32IQUERYMESSAGECP( buf, sizeof( buf ), "C:\\NO\\SUCH.MSG", &cb );
    check( "DosQueryMessageCP of a missing file fails", r != NO_ERROR, TRUE );
}

int main( void )
{
    HMQ         hmq;
    HWND        frame, server_frame;
    ULONG       flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_TASKLIST;
    ULONG       server_flags = FCF_TITLEBAR;
    QMSG        qmsg;
    PTIB        ptib;
    PPIB        ppib;
    CONVCONTEXT ctx;
    PDDESTRUCT  req;
    HSWITCH     hsw;

    /* A copy started by WinStartApp: never run the checks (or start
     * further copies) there. */
    if( starts_with_word( command_args(), "child" ) ) return child_main( command_args() );

    DosGetInfoBlocks( &ptib, &ppib );
    DosQueryModuleName( ppib->pib_hmte, sizeof( exe_path ), exe_path );

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "SwitchDdeTest", ClientWndProc, 0, 0 );
    WinRegisterClass( hab, "DdeServer", ServerWndProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "SwitchDdeTest",
                                TITLE, 0, NULLHANDLE, 0, &client );
    server_frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &server_flags, "DdeServer",
                                       "DDE Server", 0, NULLHANDLE, 0, &server_client );

    switch_checks( frame );
    atom_checks();
    msg_checks();

    /* DDE: the server answers before WinDdeInitiate returns. */
    memset( &ctx, 0, sizeof( ctx ) );
    ctx.cb = sizeof( ctx );
    ctx.fsContext = DDECTXT_CASESENSITIVE;
    check( "WinDdeInitiate", WinDdeInitiate( client, APP_NAME, TOPIC, &ctx ), TRUE );
    check( "-> the server answered", dde_server == server_client, TRUE );
    req = make_dde( ITEM, DDEFMT_TEXT, NULL, 0, 0 );
    check( "WinDdePostMsg rejects a non-DDE message",
           WinDdePostMsg( dde_server, client, WM_USER, req, 0 ), FALSE );
    check( "WinDdePostMsg(WM_DDE_REQUEST)",
           WinDdePostMsg( dde_server, client, WM_DDE_REQUEST, req, DDEPM_RETRY ), TRUE );
    check( "WinTerminateApp of a bad handle fails", WinTerminateApp( (HAPP)0x1234 ), FALSE );
    check_error( "-> PMERR_INVALID_HAPP", PMERR_INVALID_HAPP );

    WinStartTimer( hab, client, TIMEOUT_ID, TIMEOUT_MS );
    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );
    WinStopTimer( hab, client, TIMEOUT_ID );

    hsw = WinQuerySwitchHandle( frame, 0 );
    WinDestroyWindow( server_frame );
    WinDestroyWindow( frame );
    check( "the FCF_TASKLIST entry goes with its frame", WinQuerySwitchEntry( hsw, NULL ) != 0, TRUE );
    check( "-> no entries left", WinQuerySwitchList( hab, NULL, 0 ), 0 );
    finish();

    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
