/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_cnrtree_test.c - WC_CONTAINER, second stage: sorting, tree view,
 * scroll bar
 *
 * A standard frame at desktop (100, 200), 400 x 300, holds in its client a
 * container at client (8, 8), 380 x 250, in the tree view. It has twenty
 * top-level records "item 01" .. "item 20" and a "Folder" with three
 * children, all inserted out of order. The program checks that
 *
 *   CM_SORTRECORD calls the comparison function (with its pStorage) and
 *   sorts each level: Folder, then the items; child a, b, c;
 *   a sort function in CNRINFO (pSortRecord) places a record inserted at
 *   the end ("item 10b" after "item 10");
 *   CM_EXPANDTREE sends CN_EXPANDTREE before it returns;
 *
 * and leaves the folder collapsed, with 22 rows: more than the 13 that fit,
 * so the vertical scroll bar (CID_VSCROLL) is shown. Row k of the tree is
 * centred at screen y 320 + 18 k (the container's (0, 0) is screen
 * (110, 559)); the folder's expander box is at screen x 119. The
 * integration test then
 *
 *   clicks the folder's expander (119, 320)        -> CN_EXPANDTREE;
 *   clicks the scroll bar's down arrow (481, 550) twice -> CN_SCROLL;
 *
 * and closes the window with Alt+F4: the notifications, the folder's
 * expansion, the rows scrolled and the scroll bar are checked, then
 * CM_COLLAPSETREE sends CN_COLLAPSETREE. Each check is written to stdout:
 *
 *   pm_cnrtree_test: <name>: ok
 *   pm_cnrtree_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_cnrtree_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINSTDCNR
#include <os2.h>
#include <string.h>

static HAB   hab;
static HWND  frame, client, cnr;
static PRECORDCORE folder, child[3], item[21];
static int   n_checks = 0;
static int   n_passed = 0;

/* The comparison function: how often it ran, the pStorage it saw. */
static int   n_compares = 0;
static PVOID seen_storage = NULL;
static ULONG storage_word = 0x5A5A;
/* Notifications. */
static int         n_expand = 0, n_collapse = 0, n_scroll = 0;
static PRECORDCORE expand_rec = NULL, collapse_rec = NULL;
static LONG        scrolled = 0;

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
    char *p = append_str( append_str( line, "pm_cnrtree_test: " ), what );
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

static SHORT EXPENTRY Compare( PRECORDCORE p1, PRECORDCORE p2, PVOID pStorage )
{
    n_compares++;
    seen_storage = pStorage;
    return (SHORT)strcmp( p1->pszTree, p2->pszTree );
}

static PRECORDCORE query( PRECORDCORE r, USHORT cmd )
{
    return (PRECORDCORE)WinSendMsg( cnr, CM_QUERYRECORD, MPFROMP( r ), MPFROM2SHORT( cmd, CMA_ITEMORDER ) );
}

static void final_checks( void )
{
    QUERYRECORDRECT qrr;
    RECTL           r;
    HWND            bar = WinWindowFromID( cnr, CID_VSCROLL );

    check( "CN_EXPANDTREE from the expander click", n_expand, 2 );
    check( "... for the folder", expand_rec == folder, TRUE );
    check( "the folder is expanded", ( folder->flRecordAttr & CRA_EXPANDED ) != 0, TRUE );
    check( "CN_SCROLL from the scroll bar", n_scroll, 2 );
    check( "... two rows of pels", scrolled, 36 );
    check( "the scroll bar is shown", bar != NULLHANDLE && WinIsWindowVisible( bar ), TRUE );
    /* Scrolled by two rows: the folder is off the top, child b on top. */
    memset( &qrr, 0, sizeof( qrr ) );
    qrr.cb = sizeof( qrr );
    qrr.pRecord = folder;
    qrr.fsExtent = CMA_TEXT;
    check( "the folder is scrolled away", (LONG)WinSendMsg( cnr, CM_QUERYRECORDRECT, MPFROMP( &r ), MPFROMP( &qrr ) ),
           FALSE );
    qrr.pRecord = child[1];
    check( "child b is the top row", (LONG)WinSendMsg( cnr, CM_QUERYRECORDRECT, MPFROMP( &r ), MPFROMP( &qrr ) ), TRUE );
    check( "... at the top", r.yTop, 249 );
}

static MRESULT EXPENTRY ClientProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PAINT: {
        RECTL r;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &r );
        WinQueryWindowRect( hwnd, &r );
        WinFillRect( hps, &r, CLR_PALEGRAY );
        WinEndPaint( hps );
        return 0;
    }
    case WM_CONTROL:
        switch( SHORT2FROMMP( mp1 ) ) {
        case CN_EXPANDTREE:   n_expand++; expand_rec = (PRECORDCORE)mp2; break;
        case CN_COLLAPSETREE: n_collapse++; collapse_rec = (PRECORDCORE)mp2; break;
        case CN_SCROLL:       n_scroll++; scrolled += ( (PNOTIFYSCROLL)mp2 )->lScrollInc; break;
        }
        return 0;
    case WM_CLOSE:
        final_checks();
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static PRECORDCORE one_record( PSZ text )
{
    PRECORDCORE r = (PRECORDCORE)WinSendMsg( cnr, CM_ALLOCRECORD, 0, MPFROMSHORT( 1 ) );
    r->pszIcon = r->pszText = r->pszName = r->pszTree = text;
    return r;
}

static ULONG insert( PRECORDCORE r, PRECORDCORE parent )
{
    RECORDINSERT ri;
    memset( &ri, 0, sizeof( ri ) );
    ri.cb = sizeof( ri );
    ri.pRecordOrder = (PRECORDCORE)CMA_END;
    ri.pRecordParent = parent;
    ri.fInvalidateRecord = TRUE;
    ri.zOrder = CMA_TOP;
    ri.cRecordsInsert = 1;
    return (ULONG)WinSendMsg( cnr, CM_INSERTRECORD, MPFROMP( r ), MPFROMP( &ri ) );
}

static void build_container( void )
{
    static char names[20][8];
    static const int scramble[20] = { 7, 19, 2, 13, 0, 11, 5, 17, 9, 3, 15, 1, 18, 6, 12, 4, 16, 10, 8, 14 };
    static const char *kids[3] = { "child c", "child a", "child b" };
    CNRINFO     ci;
    PRECORDCORE p;
    ULONG       count;
    int         i, ok;

    cnr = WinCreateWindow( client, WC_CONTAINER, "", WS_VISIBLE,
                           8, 8, 380, 250, client, HWND_TOP, 1, NULL, NULL );
    check( "the container is created", cnr != NULLHANDLE, TRUE );
    memset( &ci, 0, sizeof( ci ) );
    ci.cb = sizeof( ci );
    ci.flWindowAttr = CV_TREE | CV_TEXT;
    WinSendMsg( cnr, CM_SETCNRINFO, MPFROMP( &ci ), MPFROMLONG( CMA_FLWINDOWATTR ) );

    for( i = 0; i < 20; i++ ) {
        int k = scramble[i] + 1;
        strcpy( names[i], "item 00" );
        names[i][5] = (char)( '0' + k / 10 );
        names[i][6] = (char)( '0' + k % 10 );
        item[k - 1] = one_record( names[i] );
        insert( item[k - 1], NULL );
    }
    folder = one_record( "Folder" );
    insert( folder, NULL );
    for( i = 0; i < 3; i++ ) {
        child[kids[i][6] - 'a'] = one_record( (PSZ)kids[i] );
        insert( child[kids[i][6] - 'a'], folder );
    }
    check( "unsorted: item 08 first", query( NULL, CMA_FIRST ) == item[7], TRUE );

    /* CM_SORTRECORD calls Compare for each comparison. */
    check( "CM_SORTRECORD", (LONG)WinSendMsg( cnr, CM_SORTRECORD, MPFROMP( Compare ), MPFROMP( &storage_word ) ),
           TRUE );
    check( "the comparison function ran", n_compares >= 20, TRUE );
    check( "... with pStorage", seen_storage == &storage_word, TRUE );
    check( "sorted: the folder first", query( NULL, CMA_FIRST ) == folder, TRUE );
    p = query( folder, CMA_NEXT );
    ok = TRUE;
    for( i = 0; i < 20; i++ ) {
        if( p != item[i] ) ok = FALSE;
        p = p ? query( p, CMA_NEXT ) : NULL;
    }
    check( "sorted: item 01 .. item 20", ok && p == NULL, TRUE );
    check( "sorted children", query( folder, CMA_FIRSTCHILD ) == child[0]
                              && query( child[0], CMA_NEXT ) == child[1] && query( child[1], CMA_NEXT ) == child[2], TRUE );

    /* A sort function in CNRINFO places new records. */
    ci.pSortRecord = (PVOID)Compare;
    WinSendMsg( cnr, CM_SETCNRINFO, MPFROMP( &ci ), MPFROMLONG( CMA_PSORTRECORD ) );
    n_compares = 0;
    item[20] = one_record( "item 10b" );
    count = insert( item[20], NULL );
    check( "insert with pSortRecord", (LONG)count, 25 );
    check( "... compared", n_compares > 0, TRUE );
    check( "... placed after item 10", query( item[9], CMA_NEXT ) == item[20], TRUE );

    /* CM_EXPANDTREE notifies before it returns; collapse it again. */
    check( "CM_EXPANDTREE", (LONG)WinSendMsg( cnr, CM_EXPANDTREE, MPFROMP( folder ), 0 ), TRUE );
    check( "CN_EXPANDTREE already received", n_expand, 1 );
    WinSendMsg( cnr, CM_COLLAPSETREE, MPFROMP( folder ), 0 );
    check( "CN_COLLAPSETREE", n_collapse, 1 );
    check( "the folder is collapsed", ( folder->flRecordAttr & CRA_EXPANDED ) == 0, TRUE );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "CnrTreeTest", ClientProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "CnrTreeTest",
                                "Container tree", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    build_container();

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    check( "CM_COLLAPSETREE after the session", (LONG)WinSendMsg( cnr, CM_COLLAPSETREE, MPFROMP( folder ), 0 ), TRUE );
    check( "... CN_COLLAPSETREE", n_collapse, 2 );
    check( "... for the folder", collapse_rec == folder, TRUE );

    WinDestroyWindow( frame );
    append_str( append_long( append_str( append_long( append_str( line, "pm_cnrtree_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
