/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_xdrag_test.c - drag and drop between programs, under Warpine
 * (ADR-0057)
 *
 * The program starts itself again as a child (the argument "child" in
 * pib_pchcmd, never in argv). The parent shows "Drag A" at the left, the
 * child "Drag B" at the right; the child publishes its windows in named
 * shared memory. The GUI script then drags with button 2 from A's client
 * to B's client. A's WM_BEGINDRAG makes one item ("Plain Text",
 * <DRM_OS2FILE,DRF_TEXT>, "hello.txt") and calls DrgDrag, which:
 *
 *   - sends DM_DRAGOVER to A's own client first (A answers DOR_NODROP),
 *     then DM_DRAGLEAVE when the pointer leaves it;
 *   - sends DM_DRAGOVER to B's client, in the other program: B reads the
 *     DRAGINFO (DrgAccessDraginfo, the item, its type, RMF and source name
 *     through the session's string handles, hwndSource another process's
 *     window) and answers DOR_DROP;
 *   - on the release sends DM_DROP to B, which asks A to render the item:
 *     DrgSendTransferMsg(DM_RENDER) with a DRAGTRANSFER, run by A's
 *     dragging thread while it waits for B's answer; B then frees the
 *     DRAGTRANSFER, deletes the item's string handles and frees its
 *     access to the DRAGINFO;
 *   - returns B's client to A, which frees the DRAGINFO.
 *
 * Each check is written to stdout:
 *
 *   pm_xdrag_test: <name>: ok
 *   pm_xdrag_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_xdrag_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_WINSTDDRAG
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include <stdio.h>

#define SHARED_NAME "\\SHAREMEM\\WARPINE_XDRAG"
#define ID_POLL     1
#define POLL_MS     100
#define WAIT_POLLS  50            /* 5 s for anything asynchronous */
#define DRAG_POLLS  250           /* the drag waits for the script */
#define TYPE        "Plain Text"
#define RMF         "<DRM_OS2FILE,DRF_TEXT>"
#define SOURCE_NAME "hello.txt"
#define RENDER_TO   "C:\\target.txt"

typedef struct {
    ULONG child_frame;
    ULONG child_client;
    LONG  child_ready;
    LONG  stop;
    LONG  child_done;
    /* What B saw at its first DM_DRAGOVER. */
    LONG  over_count;
    LONG  over_access;
    LONG  over_items;
    LONG  over_type;
    LONG  over_rmf;
    char  over_name[32];
    LONG  over_source_foreign;
    LONG  over_in_send;
    /* At DM_DROP. */
    LONG  dropped;
    LONG  drop_inside;
    LONG  render_reply;
    LONG  transfer_freed;
    LONG  info_freed;
} SHARED;

static HAB     hab;
static HWND    frame, client;
static SHARED *sh;
static int     is_child;
static int     step = 0;
static int     waited = 0;
static int     n_checks = 0, n_passed = 0;
/* The parent's side of the drag. */
static int     dragged = 0;
static HWND    drag_target = NULLHANDLE;
static LONG    own_over = 0, own_leave = 0;
static LONG    render_seen = 0, render_names = 0, render_in_send = 0;
static ULONG   free_ok = 0, status_after = 1, type_len_after = 99;

static void write_stdout( const char *s )
{
    ULONG written;
    DosWrite( 1, (PVOID)s, strlen( s ), &written );
}

static void report( const char *what, BOOL ok, const char *got, const char *want )
{
    char line[200];
    if( ok ) {
        sprintf( line, "pm_xdrag_test: %s: ok\r\n", what );
        n_passed++;
    } else {
        sprintf( line, "pm_xdrag_test: %s: FAIL got %s want %s\r\n", what, got, want );
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

static void finish( void )
{
    WinStopTimer( hab, client, ID_POLL );
    WinPostMsg( client, WM_QUIT, 0, 0 );
}

/* A's WM_BEGINDRAG: one item, dragged until the drop. */
static void begin_drag( HWND hwnd )
{
    PDRAGINFO pdinfo;
    DRAGITEM  item;
    DRAGIMAGE img;
    HSTR      htype;

    pdinfo = DrgAllocDraginfo( 1 );
    if( pdinfo == NULL ) return;
    memset( &item, 0, sizeof( item ) );
    item.hwndItem          = hwnd;
    item.ulItemID          = 1;
    item.hstrType          = htype = DrgAddStrHandle( TYPE );
    item.hstrRMF           = DrgAddStrHandle( RMF );
    item.hstrContainerName = DrgAddStrHandle( "C:\\" );
    item.hstrSourceName    = DrgAddStrHandle( SOURCE_NAME );
    item.hstrTargetName    = DrgAddStrHandle( SOURCE_NAME );
    item.fsSupportedOps    = DO_COPYABLE;
    DrgSetDragitem( pdinfo, &item, sizeof( item ), 0 );
    memset( &img, 0, sizeof( img ) );
    img.cb     = sizeof( img );
    img.hImage = WinQuerySysPointer( HWND_DESKTOP, SPTR_FILE, FALSE );
    img.fl     = DRG_ICON;
    dragged = 1;
    drag_target = DrgDrag( hwnd, pdinfo, &img, 1, VK_ENDDRAG, NULL );
    status_after = DrgQueryDragStatus();
    /* B deleted the item's handles: the session's strings are gone. */
    type_len_after = DrgQueryStrNameLen( htype );
    free_ok = DrgFreeDraginfo( pdinfo );
}

/* A's DM_RENDER from B: the DRAGTRANSFER names what to render. */
static MRESULT render( PDRAGTRANSFER pdxfer )
{
    char to[64], from[64];

    render_seen++;
    render_in_send = WinInSendMsg( hab );
    to[0] = from[0] = '\0';
    DrgQueryStrName( pdxfer->hstrRenderToName, sizeof( to ), to );
    DrgQueryStrName( pdxfer->pditem->hstrSourceName, sizeof( from ), from );
    render_names = strcmp( to, RENDER_TO ) == 0 && strcmp( from, SOURCE_NAME ) == 0;
    return (MRESULT)TRUE;
}

/* B's first DM_DRAGOVER: what the DRAGINFO from the other program holds. */
static void look_at( PDRAGINFO pdinfo )
{
    PDRAGITEM item;
    PID       pid = 0;
    TID       tid = 0;

    sh->over_access = DrgAccessDraginfo( pdinfo );
    sh->over_items  = DrgQueryDragitemCount( pdinfo );
    item = DrgQueryDragitemPtr( pdinfo, 0 );
    if( item == NULL ) return;
    sh->over_type = DrgVerifyType( item, TYPE );
    sh->over_rmf  = DrgVerifyRMF( item, "DRM_OS2FILE", NULL );
    DrgQueryStrName( item->hstrSourceName, sizeof( sh->over_name ), sh->over_name );
    WinQueryWindowProcess( pdinfo->hwndSource, &pid, &tid );
    sh->over_source_foreign = pid != 0 && pid != own_pid();
    sh->over_in_send = WinInSendMsg( hab );
}

/* B's DM_DROP: ask the source to render the item, then let go. */
static void drop( HWND hwnd, PDRAGINFO pdinfo )
{
    PDRAGITEM     item;
    PDRAGTRANSFER pdxfer;
    SWP           swp;

    DrgAccessDraginfo( pdinfo );
    WinQueryWindowPos( frame, &swp );
    sh->drop_inside = pdinfo->xDrop >= swp.x && pdinfo->xDrop < swp.x + swp.cx
                   && pdinfo->yDrop >= swp.y && pdinfo->yDrop < swp.y + swp.cy;
    item = DrgQueryDragitemPtr( pdinfo, 0 );
    pdxfer = DrgAllocDragtransfer( 1 );
    if( item != NULL && pdxfer != NULL ) {
        pdxfer->hwndClient       = hwnd;
        pdxfer->pditem           = item;
        pdxfer->hstrSelectedRMF  = DrgAddStrHandle( RMF );
        pdxfer->hstrRenderToName = DrgAddStrHandle( RENDER_TO );
        pdxfer->usOperation      = DO_COPY;
        sh->render_reply = (LONG)DrgSendTransferMsg( pdinfo->hwndSource, DM_RENDER, (MPARAM)pdxfer, 0 );
        DrgDeleteStrHandle( pdxfer->hstrSelectedRMF );
        DrgDeleteStrHandle( pdxfer->hstrRenderToName );
        sh->transfer_freed = DrgFreeDragtransfer( pdxfer );
    }
    DrgDeleteDraginfoStrHandles( pdinfo );
    sh->info_freed = DrgFreeDraginfo( pdinfo );
    sh->dropped = 1;
}

/* The parent's steps, one poll at a time. */
static void parent_poll( void )
{
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
        step = 1;
        waited = 0;
        return;
    case 1:                                     /* the script drags from A to B */
        if( !( dragged && sh->dropped ) && waited <= DRAG_POLLS ) return;
        check( "DrgDrag returned the other program's window", drag_target == (HWND)sh->child_client, TRUE );
        check( "  the source's client got DM_DRAGOVER first", own_over > 0, TRUE );
        check( "  and DM_DRAGLEAVE when the pointer left it", own_leave > 0, TRUE );
        check( "the other program got DM_DRAGOVER", sh->over_count > 0, TRUE );
        check( "  DrgAccessDraginfo on the source's DRAGINFO", sh->over_access, TRUE );
        check( "  DrgQueryDragitemCount", sh->over_items, 1 );
        check( "  DrgVerifyType", sh->over_type, TRUE );
        check( "  DrgVerifyRMF", sh->over_rmf, TRUE );
        check_str( "  DrgQueryStrName of the source's handle", sh->over_name, SOURCE_NAME );
        check( "  hwndSource is another process's window", sh->over_source_foreign, TRUE );
        check( "  sent from another program (WinInSendMsg)", sh->over_in_send, TRUE );
        check( "the other program got DM_DROP", sh->dropped, 1 );
        check( "  xDrop / yDrop lie in its frame", sh->drop_inside, TRUE );
        check( "DM_RENDER from the target ran while DrgDrag waited", render_seen, 1 );
        check( "  its DRAGTRANSFER names the item and the target file", render_names, TRUE );
        check( "  WinInSendMsg", render_in_send, TRUE );
        check( "  DrgSendTransferMsg returned the source's reply", sh->render_reply, TRUE );
        check( "the target freed the DRAGTRANSFER", sh->transfer_freed, TRUE );
        check( "the target freed its access to the DRAGINFO", sh->info_freed, TRUE );
        check( "the item's string handles are gone for the source too", (LONG)type_len_after, 0 );
        check( "DrgQueryDragStatus after the drop", (LONG)status_after, 0 );
        check( "the source freed the DRAGINFO", (LONG)free_ok, TRUE );
        sh->stop = 1;
        step = 2;
        waited = 0;
        return;
    case 2:                                     /* the child ends */
        if( !sh->child_done && waited <= WAIT_POLLS ) return;
        check( "the child ended", sh->child_done, 1 );
        finish();
        return;
    }
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_BEGINDRAG:
        if( !is_child && !dragged ) begin_drag( hwnd );
        return (MRESULT)TRUE;
    case DM_DRAGOVER:
        if( !is_child ) {
            own_over++;
            return MRFROM2SHORT( DOR_NODROP, DO_COPY );
        }
        if( sh->over_count++ == 0 ) look_at( (PDRAGINFO)mp1 );
        return MRFROM2SHORT( DOR_DROP, DO_COPY );
    case DM_DRAGLEAVE:
        if( !is_child ) own_leave++;
        return 0;
    case DM_DROP:
        if( is_child ) drop( hwnd, (PDRAGINFO)mp1 );
        return 0;
    case DM_RENDER:
        if( !is_child ) return render( (PDRAGTRANSFER)mp1 );
        return (MRESULT)FALSE;

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
    WinRegisterClass( hab, "XDragTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "XDragTest",
                                is_child ? "Drag B" : "Drag A", 0, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, is_child ? 500 : 40, 400, 300, 200,
                     SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    WinStartTimer( hab, client, ID_POLL, POLL_MS );
    if( is_child ) {
        sh->child_frame = frame;
        sh->child_client = client;
        sh->child_ready = 1;
    } else if( start_child() != NO_ERROR ) {
        write_stdout( "pm_xdrag_test: cannot start the child\r\n" );
    }

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    if( !is_child ) {
        sprintf( line, "pm_xdrag_test: done %d/%d\r\n", n_passed, n_checks );
        write_stdout( line );
    }
    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
