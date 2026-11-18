/*
 * pm_drag_test.c - drag and drop within a process (PMDRAG)
 *
 * A standard frame at desktop (100, 200), 400 x 300. Its client (screen
 * (102, 567) is client (0, 0); screen y = 567 - y) is the drag source on
 * its left half and accepts drops only on its right half; a container at
 * client (200, 8), 190 x 250, in the text view, is a second target and a
 * source of its own records.
 *
 * Before the session the program checks string handles, DrgAllocDraginfo,
 * DrgSetDragitem / DrgQueryDragitem / DrgQueryDragitemPtr, the DrgVerify*
 * calls, DrgQueryNativeRMF and DrgQueryDragStatus. The integration test
 * then drags with button 2:
 *
 *   1. from the client's left half (152, 467) onto the container
 *      (352, 467): WM_BEGINDRAG -> DrgDrag; CN_DRAGOVER accepts a
 *      "Plain Text" item; CN_DROP inserts a record named after the item's
 *      source name ("dropped.txt"); DrgFreeDraginfo inside the drop fails
 *      with PMERR_SOURCE_SAME_AS_TARGET; DrgDrag returns the container.
 *   2. from that record (row 0 of the container, screen 322, 319) onto
 *      the client's left half (130, 520): CN_INITDRAG names the record;
 *      DM_DRAGOVER on the left half answers DOR_NEVERDROP; DrgDrag returns
 *      NULLHANDLE after DM_DRAGLEAVE.
 *
 * and closes the window with Alt+F4: the counts and results are checked.
 * Each check is written to stdout:
 *
 *   pm_drag_test: <name>: ok
 *   pm_drag_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_drag_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINSTDDRAG
#define INCL_WINSTDCNR
#define INCL_SHLERRORS
#include <os2.h>
#include <pmerr.h>
#include <string.h>

static HAB   hab;
static HWND  frame, client, cnr;
static int   n_checks = 0;
static int   n_passed = 0;

/* What the session saw. */
static int         n_begin = 0, n_initdrag = 0, n_cnr_over = 0, n_cnr_drop = 0;
static int         n_over_left = 0, n_leave = 0, n_drop_client = 0;
static HWND        drag1_result = (HWND)-1, drag2_result = (HWND)-1;
static ULONG       free_in_drop = 1, free_error = 0;
static PRECORDCORE init_rec = NULL;
static PRECORDCORE dropped = NULL;
static char        dropped_name[32];

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
    char *p = append_str( append_str( line, "pm_drag_test: " ), what );
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

/* A DRAGINFO with one "Plain Text" item named `name`. */
static PDRAGINFO make_draginfo( HWND source, PSZ name )
{
    DRAGITEM  di;
    PDRAGINFO pdi = DrgAllocDraginfo( 1 );
    memset( &di, 0, sizeof( di ) );
    di.hwndItem = source;
    di.ulItemID = 1;
    di.hstrType = DrgAddStrHandle( "Plain Text" );
    di.hstrRMF = DrgAddStrHandle( "<DRM_SHAREDMEM,DRF_TEXT>" );
    di.hstrContainerName = DrgAddStrHandle( "C:\\" );
    di.hstrSourceName = DrgAddStrHandle( name );
    di.hstrTargetName = DrgAddStrHandle( name );
    di.fsSupportedOps = DO_COPYABLE | DO_MOVEABLE;
    DrgSetDragitem( pdi, &di, sizeof( di ), 0 );
    pdi->hwndSource = source;
    return pdi;
}

static HWND drag( HWND source, PSZ name )
{
    DRAGIMAGE img;
    PDRAGINFO pdi = make_draginfo( source, name );
    HWND      target;
    memset( &img, 0, sizeof( img ) );
    img.cb = sizeof( img );
    img.hImage = WinQuerySysPointer( HWND_DESKTOP, SPTR_FILE, FALSE );
    img.fl = DRG_ICON;
    target = DrgDrag( source, pdi, &img, 1, VK_ENDDRAG, NULL );
    DrgFreeDraginfo( pdi );
    return target;
}

static MRESULT accept_text( PDRAGINFO pdi )
{
    PDRAGITEM pdit;
    if( !DrgAccessDraginfo( pdi ) ) return MRFROM2SHORT( DOR_NEVERDROP, 0 );
    pdit = DrgQueryDragitemPtr( pdi, 0 );
    if( pdit && DrgVerifyType( pdit, "Plain Text" ) ) return MRFROM2SHORT( DOR_DROP, DO_COPY );
    return MRFROM2SHORT( DOR_NEVERDROP, 0 );
}

static void insert_record( PSZ name )
{
    RECORDINSERT ri;
    dropped = (PRECORDCORE)WinSendMsg( cnr, CM_ALLOCRECORD, 0, MPFROMSHORT( 1 ) );
    dropped->pszIcon = dropped->pszText = dropped->pszName = dropped->pszTree = name;
    memset( &ri, 0, sizeof( ri ) );
    ri.cb = sizeof( ri );
    ri.pRecordOrder = (PRECORDCORE)CMA_FIRST;
    ri.fInvalidateRecord = TRUE;
    ri.zOrder = CMA_TOP;
    ri.cRecordsInsert = 1;
    WinSendMsg( cnr, CM_INSERTRECORD, MPFROMP( dropped ), MPFROMP( &ri ) );
}

static void final_checks( void )
{
    check( "WM_BEGINDRAG on the client", n_begin, 1 );
    check( "drag 1 returns the container", drag1_result == cnr, TRUE );
    check( "CN_DRAGOVER asked the owner", n_cnr_over > 0, TRUE );
    check( "CN_DROP", n_cnr_drop, 1 );
    check( "the dropped item's source name", strcmp( dropped_name, "dropped.txt" ) == 0, TRUE );
    check( "DrgFreeDraginfo in the drop fails", (LONG)free_in_drop, FALSE );
    check( "... PMERR_SOURCE_SAME_AS_TARGET", (LONG)free_error, PMERR_SOURCE_SAME_AS_TARGET );
    check( "CN_INITDRAG from the container", n_initdrag, 1 );
    check( "... names the dropped record", init_rec != NULL && init_rec == dropped, TRUE );
    check( "DM_DRAGOVER on the left half", n_over_left > 0, TRUE );
    check( "DM_DRAGLEAVE after the refused drop", n_leave > 0, TRUE );
    check( "no DM_DROP on the client", n_drop_client, 0 );
    check( "drag 2 returns NULLHANDLE", drag2_result == NULLHANDLE, TRUE );
    check( "no drag in progress", (LONG)DrgQueryDragStatus(), 0 );
}

static MRESULT EXPENTRY ClientProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
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
    case WM_BEGINDRAG:
        n_begin++;
        drag1_result = drag( hwnd, "dropped.txt" );
        return (MRESULT)TRUE;
    case DM_DRAGOVER:
        /* Only the right half takes drops; the left half never does. */
        if( SHORT1FROMMP( mp2 ) - 102 < 190 ) {
            n_over_left++;
            return MRFROM2SHORT( DOR_NEVERDROP, 0 );
        }
        return accept_text( (PDRAGINFO)mp1 );
    case DM_DRAGLEAVE:
        n_leave++;
        return 0;
    case DM_DROP:
        n_drop_client++;
        return 0;
    case WM_CONTROL:
        switch( SHORT2FROMMP( mp1 ) ) {
        case CN_DRAGOVER:
            n_cnr_over++;
            return accept_text( ( (PCNRDRAGINFO)mp2 )->pDragInfo );
        case CN_DROP: {
            PDRAGINFO pdi = ( (PCNRDRAGINFO)mp2 )->pDragInfo;
            PDRAGITEM pdit;
            n_cnr_drop++;
            DrgAccessDraginfo( pdi );
            pdit = DrgQueryDragitemPtr( pdi, 0 );
            DrgQueryStrName( pdit->hstrSourceName, sizeof( dropped_name ), dropped_name );
            insert_record( dropped_name );
            DrgDeleteDraginfoStrHandles( pdi );
            WinGetLastError( hab );
            free_in_drop = DrgFreeDraginfo( pdi );
            free_error = ERRORIDERROR( WinGetLastError( hab ) );
            return 0;
        }
        case CN_INITDRAG: {
            PCNRDRAGINIT pi = (PCNRDRAGINIT)mp2;
            n_initdrag++;
            init_rec = pi->pRecord;
            drag2_result = drag( cnr, init_rec ? init_rec->pszText : "white space" );
            return 0;
        }
        }
        return 0;
    case WM_CLOSE:
        final_checks();
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static void data_checks( void )
{
    PDRAGINFO pdi = make_draginfo( client, "data.txt" );
    PDRAGITEM pdit;
    DRAGITEM  copy;
    char      buf[64];

    check( "DrgAllocDraginfo", pdi != NULL, TRUE );
    check( "cbDragitem", pdi->cbDragitem, sizeof( DRAGITEM ) );
    check( "cditem", pdi->cditem, 1 );
    check( "DrgQueryDragitemCount", (LONG)DrgQueryDragitemCount( pdi ), 1 );
    pdit = DrgQueryDragitemPtr( pdi, 0 );
    check( "DrgQueryDragitemPtr", pdit != NULL, TRUE );
    check( "DrgQueryDragitem", DrgQueryDragitem( pdi, sizeof( copy ), &copy, 0 ), TRUE );
    check( "... the same item", copy.hstrSourceName == pdit->hstrSourceName && copy.ulItemID == 1, TRUE );
    check( "DrgQueryStrNameLen", (LONG)DrgQueryStrNameLen( pdit->hstrSourceName ), 8 );
    DrgQueryStrName( pdit->hstrSourceName, sizeof( buf ), buf );
    check( "DrgQueryStrName", strcmp( buf, "data.txt" ) == 0, TRUE );
    check( "DrgVerifyType", DrgVerifyType( pdit, "Bitmap,Plain Text" ), TRUE );
    check( "DrgVerifyTrueType", DrgVerifyTrueType( pdit, "Plain Text" ), TRUE );
    check( "DrgVerifyRMF", DrgVerifyRMF( pdit, "DRM_SHAREDMEM", NULL ), TRUE );
    check( "DrgVerifyRMF (other format)", DrgVerifyRMF( pdit, NULL, "DRF_BITMAP" ), FALSE );
    check( "DrgQueryNativeRMF", DrgQueryNativeRMF( pdit, sizeof( buf ), buf ), TRUE );
    check( "... is the first pair", strcmp( buf, "<DRM_SHAREDMEM,DRF_TEXT>" ) == 0, TRUE );
    check( "DrgQueryDragStatus", (LONG)DrgQueryDragStatus(), 0 );
    check( "DrgDeleteDraginfoStrHandles", DrgDeleteDraginfoStrHandles( pdi ), TRUE );
    check( "DrgFreeDraginfo", DrgFreeDraginfo( pdi ), TRUE );
}

int main( void )
{
    HMQ     hmq;
    QMSG    qmsg;
    ULONG   flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    CNRINFO ci;
    char    line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "DragTest", ClientProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "DragTest",
                                "Drag and drop", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    cnr = WinCreateWindow( client, WC_CONTAINER, "", WS_VISIBLE,
                           200, 8, 190, 250, client, HWND_TOP, 1, NULL, NULL );
    memset( &ci, 0, sizeof( ci ) );
    ci.cb = sizeof( ci );
    ci.flWindowAttr = CV_TEXT;
    WinSendMsg( cnr, CM_SETCNRINFO, MPFROMP( &ci ), MPFROMLONG( CMA_FLWINDOWATTR ) );
    data_checks();

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( frame );
    append_str( append_long( append_str( append_long( append_str( line, "pm_drag_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
