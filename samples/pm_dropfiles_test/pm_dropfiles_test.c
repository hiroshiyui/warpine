/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_dropfiles_test.c - files dragged and dropped (PMDRAG), the drag's
 * operation keys, WM_ENDDRAG and the default procedure passing drag and
 * context-menu messages to the owner
 *
 * A standard frame at desktop (100, 200), 400 x 300. Its client (screen
 * (102, 567) is client (0, 0); screen y = 567 - y) takes dropped files on
 * its right half (x >= 190) and never on its left half, which is where its
 * own drags start. A child window of class "Child" at client (20, 200),
 * 100 x 60 (screen (122, 307) - (222, 367)), owned by the client, leaves
 * every message to WinDefWindowProc.
 *
 * Before the session the program checks DrgQueryDraginfoPtr,
 * DrgQueryDraginfoPtrFromHwnd and DrgReallocDraginfo. The integration test
 * puts C:\DROPME.TXT ("dropped!") on drive C, then:
 *
 *   1. drops the host file C:\DROPME.TXT stands for on the right half: a
 *      DRAGINFO of one <DRM_OS2FILE,DRF_UNKNOWN> item without a source
 *      window, DM_DRAGOVER (accepted) then DM_DROP; the file is read
 *      through the container and source names;
 *   2. drops a host directory outside the drives: nothing arrives;
 *   3. drops the file on the left half: DM_DRAGOVER refused, DM_DRAGLEAVE;
 *   4. drags with button 2 and Ctrl held from the left half (152, 467) to
 *      the right half (352, 467): WM_BEGINDRAG -> DrgDragFiles of
 *      C:\DROPME.TXT as "Plain Text"; DM_DRAGOVER sees DO_COPY and the
 *      client as the source; DrgDragFiles returns TRUE;
 *   5. clicks button 2 on the child: its WM_CONTEXTMENU reaches the client
 *      through WinDefWindowProc;
 *   6. drags with button 2 inside the child: WM_BEGINDRAG reaches the
 *      client the same way, which starts no drag; the release brings
 *      WM_ENDDRAG to the child and from it to the client;
 *
 * and closes the window with Alt+F4: the counts and results are checked.
 * Each check is written to stdout:
 *
 *   pm_dropfiles_test: <name>: ok
 *   pm_dropfiles_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_dropfiles_test: done <passed>/<total>".
 */
#define INCL_DOS
#define INCL_WIN
#define INCL_GPI
#define INCL_WINSTDDRAG
#include <os2.h>
#include <string.h>

static HAB   hab;
static HWND  frame, client, child;
static int   n_checks = 0;
static int   n_passed = 0;

/* What the session saw. */
static int   n_host_over = 0, n_host_drop = 0, n_host_leave = 0, n_left_over = 0;
static int   host_type_ok = 0, host_rmf_ok = 0, host_op = -1, host_ops = -1, host_ctl = -1;
static HWND  host_source = (HWND)-1;
static char  host_path[64];
static char  host_text[16];
static ULONG host_free = 0;
static int   n_begin = 0, n_files_over = 0, n_files_drop = 0;
static int   files_op = -1, files_type_ok = 0;
static HWND  files_source = (HWND)-1;
static char  files_name[32];
static BOOL  files_result = 2;
static int   n_child_ctx = 0, n_client_ctx = 0, n_child_begin = 0, n_begin_forwarded = 0;
static int   n_child_end = 0, n_end_forwarded = 0;
static int   child_forwarding = 0;

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
    char *p = append_str( append_str( line, "pm_dropfiles_test: " ), what );
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

/* The full name of the item's file: container + source name. */
static void item_path( PDRAGITEM pdit, char *buf, ULONG cb )
{
    ULONG n = DrgQueryStrName( pdit->hstrContainerName, cb, buf );
    DrgQueryStrName( pdit->hstrSourceName, cb - n, buf + n );
}

/* Read the start of a file into buf (NUL-terminated). */
static void read_file( const char *path, char *buf, ULONG cb )
{
    HFILE h;
    ULONG action, got = 0;
    buf[0] = '\0';
    if( DosOpen( (PSZ)path, &h, &action, 0, FILE_NORMAL, OPEN_ACTION_FAIL_IF_NEW | OPEN_ACTION_OPEN_IF_EXISTS,
                 OPEN_ACCESS_READONLY | OPEN_SHARE_DENYNONE, NULL ) != 0 ) return;
    DosRead( h, buf, cb - 1, &got );
    buf[got] = '\0';
    DosClose( h );
}

static void final_checks( void )
{
    check( "host drop: DM_DRAGOVER", n_host_over, 1 );
    check( "... the type is Unknown", host_type_ok, 1 );
    check( "... DRM_OS2FILE is the native RMF", host_rmf_ok, 1 );
    check( "... usOperation DO_DEFAULT", host_op, DO_DEFAULT );
    check( "... copy and move supported", host_ops, DO_COPYABLE | DO_MOVEABLE );
    check( "... a file, not a container", host_ctl, 0 );
    check( "... no source window", host_source == NULLHANDLE, TRUE );
    check( "host drop: DM_DROP", n_host_drop, 1 );
    check( "... names the file", strcmp( host_path, "C:\\DROPME.TXT" ) == 0, TRUE );
    check( "... which reads back", strcmp( host_text, "dropped!" ) == 0, TRUE );
    check( "... and the target frees the DRAGINFO", (LONG)host_free, TRUE );
    check( "a directory outside the drives never arrives", n_host_over + n_left_over, 2 );
    check( "a refused host drop: DM_DRAGOVER on the left half", n_left_over, 1 );
    check( "... and DM_DRAGLEAVE", n_host_leave, 1 );
    check( "WM_BEGINDRAG on the client", n_begin, 1 );
    check( "DrgDragFiles: DM_DRAGOVER", n_files_over > 0, TRUE );
    check( "... Ctrl held is DO_COPY", files_op, DO_COPY );
    check( "... the client is the source", files_source == client, TRUE );
    check( "... the given type", files_type_ok, 1 );
    check( "DrgDragFiles: DM_DROP", n_files_drop, 1 );
    check( "... names the file", strcmp( files_name, "DROPME.TXT" ) == 0, TRUE );
    check( "DrgDragFiles returns TRUE", files_result, TRUE );
    check( "WM_CONTEXTMENU on the child", n_child_ctx, 1 );
    check( "... reaches its owner", n_client_ctx, 1 );
    check( "WM_BEGINDRAG on the child", n_child_begin, 1 );
    check( "... reaches its owner", n_begin_forwarded, 1 );
    check( "WM_ENDDRAG on the child", n_child_end, 1 );
    check( "... reaches its owner", n_end_forwarded, 1 );
    check( "no drag in progress", (LONG)DrgQueryDragStatus(), 0 );
}

static MRESULT host_over( PDRAGINFO pdi, MPARAM mp2 )
{
    PDRAGITEM pdit;
    if( !DrgAccessDraginfo( pdi ) ) return MRFROM2SHORT( DOR_NEVERDROP, 0 );
    if( SHORT1FROMMP( mp2 ) - 102 < 190 ) {
        n_left_over++;
        return MRFROM2SHORT( DOR_NODROP, 0 );
    }
    pdit = DrgQueryDragitemPtr( pdi, 0 );
    n_host_over++;
    host_type_ok = DrgVerifyTrueType( pdit, "Unknown" );
    host_rmf_ok = DrgVerifyNativeRMF( pdit, "<DRM_OS2FILE,DRF_UNKNOWN>" );
    host_op = pdi->usOperation;
    host_ops = pdit->fsSupportedOps;
    host_ctl = pdit->fsControl;
    host_source = pdi->hwndSource;
    return MRFROM2SHORT( DOR_DROP, DO_COPY );
}

static MRESULT files_over( PDRAGINFO pdi, MPARAM mp2 )
{
    PDRAGITEM pdit;
    if( !DrgAccessDraginfo( pdi ) ) return MRFROM2SHORT( DOR_NEVERDROP, 0 );
    if( SHORT1FROMMP( mp2 ) - 102 < 190 ) return MRFROM2SHORT( DOR_NODROP, 0 );
    pdit = DrgQueryDragitemPtr( pdi, 0 );
    n_files_over++;
    files_op = pdi->usOperation;
    files_source = pdi->hwndSource;
    files_type_ok = DrgVerifyTrueType( pdit, "Plain Text" );
    return MRFROM2SHORT( DOR_DROP, DO_COPY );
}

static MRESULT EXPENTRY ClientProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    static int dragging_files = 0;
    switch( msg ) {
    case WM_PAINT: {
        RECTL r;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &r );
        WinQueryWindowRect( hwnd, &r );
        WinFillRect( hps, &r, CLR_PALEGRAY );
        r.xRight = 190;
        WinFillRect( hps, &r, CLR_DARKGRAY );
        WinEndPaint( hps );
        return 0;
    }
    case WM_BEGINDRAG: {
        PCSZ files[1], types[1];
        if( child_forwarding ) {
            n_begin_forwarded++;
            return (MRESULT)TRUE;
        }
        n_begin++;
        files[0] = "C:\\DROPME.TXT";
        types[0] = "Plain Text";
        dragging_files = 1;
        files_result = DrgDragFiles( hwnd, files, types, NULL, 1, NULLHANDLE, VK_ENDDRAG, FALSE, 0 );
        dragging_files = 0;
        return (MRESULT)TRUE;
    }
    case WM_ENDDRAG:
        n_end_forwarded++;
        return (MRESULT)TRUE;
    case WM_CONTEXTMENU:
        n_client_ctx++;
        return (MRESULT)TRUE;
    case DM_DRAGOVER:
        return dragging_files ? files_over( (PDRAGINFO)mp1, mp2 ) : host_over( (PDRAGINFO)mp1, mp2 );
    case DM_DRAGLEAVE:
        if( !dragging_files ) n_host_leave++;
        return 0;
    case DM_DROP: {
        PDRAGINFO pdi = (PDRAGINFO)mp1;
        PDRAGITEM pdit;
        DrgAccessDraginfo( pdi );
        pdit = DrgQueryDragitemPtr( pdi, 0 );
        if( dragging_files ) {
            n_files_drop++;
            DrgQueryStrName( pdit->hstrSourceName, sizeof( files_name ), files_name );
        } else {
            n_host_drop++;
            item_path( pdit, host_path, sizeof( host_path ) );
            read_file( host_path, host_text, sizeof( host_text ) );
            DrgDeleteDraginfoStrHandles( pdi );
            host_free = DrgFreeDraginfo( pdi );
        }
        return 0;
    }
    case WM_CLOSE:
        final_checks();
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

/* The child counts the direct-manipulation messages and leaves them to
   WinDefWindowProc, which passes them to its owner, the client. */
static MRESULT EXPENTRY ChildProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    MRESULT r;
    switch( msg ) {
    case WM_PAINT: {
        RECTL rc;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinQueryWindowRect( hwnd, &rc );
        WinFillRect( hps, &rc, CLR_DARKCYAN );
        WinEndPaint( hps );
        return 0;
    }
    case WM_CONTEXTMENU:
        n_child_ctx++;
        break;
    case WM_BEGINDRAG:
        n_child_begin++;
        child_forwarding = 1;
        r = WinDefWindowProc( hwnd, msg, mp1, mp2 );
        child_forwarding = 0;
        return r;
    case WM_ENDDRAG:
        n_child_end++;
        break;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static void data_checks( void )
{
    PDRAGINFO a = DrgAllocDraginfo( 1 );
    PDRAGINFO b = DrgAllocDraginfo( 2 );
    PDRAGINFO lo = a < b ? a : b, hi = a < b ? b : a;
    PDRAGINFO c;
    DRAGITEM  di;

    check( "DrgQueryDraginfoPtr(NULL) is the first", DrgQueryDraginfoPtr( NULL ) == lo, TRUE );
    check( "... then the next", DrgQueryDraginfoPtr( lo ) == hi, TRUE );
    check( "... then NULL", DrgQueryDraginfoPtr( hi ) == NULL, TRUE );
    b->hwndSource = client;
    check( "DrgQueryDraginfoPtrFromHwnd", DrgQueryDraginfoPtrFromHwnd( client ) == b, TRUE );
    check( "... of no window", DrgQueryDraginfoPtrFromHwnd( frame ) == NULL, TRUE );
    memset( &di, 0, sizeof( di ) );
    di.ulItemID = 42;
    DrgSetDragitem( b, &di, sizeof( di ), 1 );
    b->usOperation = DO_MOVE;
    c = DrgReallocDraginfo( b, 3 );
    check( "DrgReallocDraginfo", c != NULL, TRUE );
    check( "... has three items", c ? c->cditem : 0, 3 );
    check( "... keeps the items", c ? DrgQueryDragitemPtr( c, 1 )->ulItemID : 0, 42 );
    check( "... and the header", c ? c->usOperation : 0, DO_MOVE );
    check( "... and the source", c ? c->hwndSource == client : 0, TRUE );
    check( "DrgFreeDraginfo of the new one", DrgFreeDraginfo( c ), TRUE );
    check( "DrgFreeDraginfo of the other", DrgFreeDraginfo( a ), TRUE );
    check( "no DRAGINFO left", DrgQueryDraginfoPtr( NULL ) == NULL, TRUE );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "DropTest", ClientProc, CS_SIZEREDRAW, 0 );
    WinRegisterClass( hab, "Child", ChildProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "DropTest",
                                "Dropped files", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    child = WinCreateWindow( client, "Child", "", WS_VISIBLE, 20, 200, 100, 60, client, HWND_TOP, 1, NULL, NULL );
    data_checks();

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( frame );
    append_str( append_long( append_str( append_long( append_str( line, "pm_dropfiles_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
