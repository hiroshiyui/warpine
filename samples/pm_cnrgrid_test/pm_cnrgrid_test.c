/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_cnrgrid_test.c - WC_CONTAINER, third stage: tree look and moves,
 * grid arrangement, erasing, stacking, context menus, help, the wheel
 *
 * A standard frame at desktop (100, 200), 500 x 340, holds in its client
 * two containers:
 *
 *   TREE (client (8, 8), 230 x 300): the tree view with tree lines
 *     (CA_TREELINE) of "Alpha" { "A1", "A2" { "A21" } }, "Beta" { 20
 *     leaves }, "Gamma"; Alpha's expander is a picture (a TREEITEMDESC with
 *     the folder icon). CM_MOVETREE moves Gamma under Alpha, first.
 *   ICONS (client (246, 8), 240 x 300): the icon view of six records, one
 *     inserted at the bottom of the stacking (RECORDINSERT.zOrder
 *     CMA_BOTTOM), arranged on a 72 x 70 grid (CM_SETGRIDINFO,
 *     CM_ARRANGE CMA_ARRANGEGRID); CM_ERASERECORD and CM_INVALIDATERECORD;
 *     CM_QUERYVIEWPORTRECT(CMA_WORKSPACE).
 *
 * The program checks those, and that WM_CONTEXTMENU and WM_HELP sent to a
 * container give its owner CN_CONTEXTMENU / CN_HELP. The integration test
 * (see tests/integration.rs, CNRGRID_STEPS) then:
 *
 *   clicks button 2 on the first icon          -> CN_CONTEXTMENU, that record;
 *   clicks "Beta" in the tree, presses Shift+F10 -> CN_CONTEXTMENU, Beta;
 *   turns the wheel down two notches over the tree -> CN_SCROLL;
 *
 * and closes the window with Alt+F4. Each check is written to stdout:
 *
 *   pm_cnrgrid_test: <name>: ok
 *   pm_cnrgrid_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_cnrgrid_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINSTDCNR
#include <os2.h>
#include <string.h>

#define ID_TREE   1
#define ID_ICONS  2
#define MAX_LOG   16

static HAB  hab;
static HWND frame, client, tree, icons;
static PRECORDCORE alpha, a2, beta, gamma, icon_rec[6];
static int  n_checks = 0;
static int  n_passed = 0;
static TREEITEMDESC alpha_desc;

/* CN_CONTEXTMENU / CN_HELP: the control and the record; CN_SCROLL count. */
static USHORT ctx_id[MAX_LOG];
static PRECORDCORE ctx_rec[MAX_LOG];
static int  n_ctx = 0, n_help = 0, n_tree_scroll = 0;
static PRECORDCORE help_rec = NULL;

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
    char *p = append_str( append_str( line, "pm_cnrgrid_test: " ), what );
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

static PRECORDCORE alloc_records( HWND cnr, int n )
{
    return (PRECORDCORE)WinSendMsg( cnr, CM_ALLOCRECORD, MPFROMLONG( 0 ), MPFROMSHORT( n ) );
}

static void name( PRECORDCORE r, PSZ text, HPOINTER icon )
{
    r->pszIcon = r->pszText = r->pszName = r->pszTree = text;
    r->hptrIcon = icon;
}

static ULONG insert( HWND cnr, PRECORDCORE first, ULONG n, PRECORDCORE parent, ULONG z )
{
    RECORDINSERT ri;
    memset( &ri, 0, sizeof( ri ) );
    ri.cb = sizeof( ri );
    ri.pRecordOrder = (PRECORDCORE)CMA_END;
    ri.pRecordParent = parent;
    ri.fInvalidateRecord = TRUE;
    ri.zOrder = (USHORT)z;
    ri.cRecordsInsert = n;
    return (ULONG)WinSendMsg( cnr, CM_INSERTRECORD, MPFROMP( first ), MPFROMP( &ri ) );
}

static PRECORDCORE parent_of( HWND cnr, PRECORDCORE r )
{
    return (PRECORDCORE)WinSendMsg( cnr, CM_QUERYRECORD, MPFROMP( r ), MPFROM2SHORT( CMA_PARENT, CMA_ITEMORDER ) );
}

static void build_tree( void )
{
    static PSZ leaves[20] = { "b01", "b02", "b03", "b04", "b05", "b06", "b07", "b08", "b09", "b10",
                              "b11", "b12", "b13", "b14", "b15", "b16", "b17", "b18", "b19", "b20" };
    HPOINTER folder = WinQuerySysPointer( HWND_DESKTOP, SPTR_FOLDER, FALSE );
    PRECORDCORE top, kids, a21, leaf;
    CNRINFO   ci;
    TREEMOVE  tm;
    int       i;

    tree = WinCreateWindow( client, WC_CONTAINER, "", WS_VISIBLE | CCS_SINGLESEL,
                            8, 8, 230, 300, client, HWND_TOP, ID_TREE, NULL, NULL );
    top = alloc_records( tree, 3 );
    alpha = top; beta = top->preccNextRecord; gamma = beta->preccNextRecord;
    name( alpha, "Alpha", NULLHANDLE ); name( beta, "Beta", NULLHANDLE ); name( gamma, "Gamma", NULLHANDLE );
    alpha_desc.hptrExpanded = folder;
    alpha_desc.hptrCollapsed = folder;
    alpha->pTreeItemDesc = &alpha_desc;
    insert( tree, top, 3, NULL, CMA_TOP );
    kids = alloc_records( tree, 2 );
    a2 = kids->preccNextRecord;
    name( kids, "A1", NULLHANDLE ); name( a2, "A2", NULLHANDLE );
    insert( tree, kids, 2, alpha, CMA_TOP );
    a21 = alloc_records( tree, 1 );
    name( a21, "A21", NULLHANDLE );
    insert( tree, a21, 1, a2, CMA_TOP );
    leaf = alloc_records( tree, 20 );
    kids = leaf;
    for( i = 0; i < 20; i++ ) { name( leaf, leaves[i], NULLHANDLE ); leaf = leaf->preccNextRecord; }
    insert( tree, kids, 20, beta, CMA_TOP );

    memset( &ci, 0, sizeof( ci ) );
    ci.cb = sizeof( ci );
    ci.flWindowAttr = CV_TREE | CV_TEXT | CA_TREELINE;
    WinSendMsg( tree, CM_SETCNRINFO, MPFROMP( &ci ), MPFROMLONG( CMA_FLWINDOWATTR ) );
    WinSendMsg( tree, CM_EXPANDTREE, 0, 0 );

    /* Gamma under Alpha, first. */
    tm.preccMove = gamma; tm.preccNewParent = alpha; tm.pRecordOrder = (PRECORDCORE)CMA_FIRST; tm.flMoveSiblings = FALSE;
    check( "CM_MOVETREE", (LONG)WinSendMsg( tree, CM_MOVETREE, MPFROMP( &tm ), 0 ), TRUE );
    check( "... Gamma is Alpha's child", parent_of( tree, gamma ) == alpha, TRUE );
    check( "... its first", (LONG)WinSendMsg( tree, CM_QUERYRECORD, MPFROMP( alpha ),
                                              MPFROM2SHORT( CMA_FIRSTCHILD, CMA_ITEMORDER ) ), (LONG)gamma );
    tm.preccMove = alpha; tm.preccNewParent = a2; tm.pRecordOrder = (PRECORDCORE)CMA_END;
    check( "CM_MOVETREE under its own child fails", (LONG)WinSendMsg( tree, CM_MOVETREE, MPFROMP( &tm ), 0 ), FALSE );
}

static void build_icons( void )
{
    static PSZ names[6] = { "One", "Two", "Three", "Four", "Five", "Under" };
    HPOINTER file = WinQuerySysPointer( HWND_DESKTOP, SPTR_FILE, FALSE );
    PRECORDCORE r;
    GRIDINFO    gi;
    CNRINFO     ci;
    QUERYRECFROMRECT q;
    RECTL       ws;
    int         i;

    icons = WinCreateWindow( client, WC_CONTAINER, "", WS_VISIBLE | CCS_EXTENDSEL,
                             246, 8, 240, 300, client, HWND_TOP, ID_ICONS, NULL, NULL );
    memset( &ci, 0, sizeof( ci ) );
    ci.cb = sizeof( ci );
    ci.flWindowAttr = CV_ICON;
    WinSendMsg( icons, CM_SETCNRINFO, MPFROMP( &ci ), MPFROMLONG( CMA_FLWINDOWATTR ) );
    r = alloc_records( icons, 5 );
    for( i = 0; i < 5; i++ ) { icon_rec[i] = r; name( r, names[i], file ); r = r->preccNextRecord; }
    insert( icons, icon_rec[0], 5, NULL, CMA_TOP );
    icon_rec[5] = alloc_records( icons, 1 );
    name( icon_rec[5], names[5], file );
    insert( icons, icon_rec[5], 1, NULL, CMA_BOTTOM );

    memset( &gi, 0, sizeof( gi ) );
    gi.cb = sizeof( gi );
    gi.cxGrid = 72; gi.cyGrid = 70;
    check( "CM_SETGRIDINFO", (LONG)WinSendMsg( icons, CM_SETGRIDINFO, MPFROMP( &gi ), 0 ), TRUE );
    memset( &gi, 0, sizeof( gi ) );
    gi.cb = sizeof( gi );
    check( "CM_QUERYGRIDINFO", (LONG)WinSendMsg( icons, CM_QUERYGRIDINFO, MPFROMP( &gi ), 0 ), TRUE );
    check( "... cxGrid", gi.cxGrid, 72 );
    check( "... columns", gi.sGridCols, 3 );
    check( "CM_ARRANGE on the grid", (LONG)WinSendMsg( icons, CM_ARRANGE, MPFROMLONG( CMA_ARRANGEGRID ), 0 ), TRUE );
    check( "... one square apart", icon_rec[1]->ptlIcon.x - icon_rec[0]->ptlIcon.x, 72 );
    check( "... a row down", icon_rec[0]->ptlIcon.y - icon_rec[3]->ptlIcon.y, 70 );

    /* Stacking: put "Under" where "One" is; One is on top there. */
    icon_rec[5]->ptlIcon = icon_rec[0]->ptlIcon;
    WinSendMsg( icons, CM_INVALIDATERECORD, 0, 0 );
    memset( &q, 0, sizeof( q ) );
    q.cb = sizeof( q );
    q.rect.xLeft = icon_rec[0]->ptlIcon.x + 2; q.rect.yBottom = icon_rec[0]->ptlIcon.y + 2;
    q.rect.xRight = q.rect.xLeft + 4; q.rect.yTop = q.rect.yBottom + 4;
    q.fsSearch = CMA_PARTIAL | CMA_ZORDER;
    check( "CMA_BOTTOM: One on top of Under", (LONG)WinSendMsg( icons, CM_QUERYRECORDFROMRECT,
           MPFROMLONG( CMA_FIRST ), MPFROMP( &q ) ), (LONG)icon_rec[0] );
    WinSendMsg( icons, CM_ARRANGE, MPFROMLONG( CMA_ARRANGEGRID ), 0 );

    check( "CM_ERASERECORD", (LONG)WinSendMsg( icons, CM_ERASERECORD, MPFROMP( icon_rec[2] ), 0 ), TRUE );
    check( "CM_INVALIDATERECORD brings it back", (LONG)WinSendMsg( icons, CM_INVALIDATERECORD, 0, 0 ), TRUE );
    check( "CM_QUERYVIEWPORTRECT (CMA_WORKSPACE)", (LONG)WinSendMsg( icons, CM_QUERYVIEWPORTRECT, MPFROMP( &ws ),
           MPFROM2SHORT( CMA_WORKSPACE, FALSE ) ), TRUE );
    check( "... holds the last square", ws.xRight >= icon_rec[2]->ptlIcon.x + 32 && ws.yTop >= icon_rec[0]->ptlIcon.y, TRUE );

    /* WM_CONTEXTMENU and WM_HELP sent to the container. */
    WinSendMsg( icons, CM_SETRECORDEMPHASIS, MPFROMP( icon_rec[4] ), MPFROM2SHORT( TRUE, CRA_CURSORED ) );
    WinSendMsg( icons, WM_HELP, 0, 0 );
    check( "WM_HELP: CN_HELP", n_help, 1 );
    check( "... with the cursored record", (LONG)help_rec, (LONG)icon_rec[4] );
    n_ctx = 0;
}

static void final_checks( void )
{
    check( "context menus", n_ctx, 2 );
    check( "button 2 on One: the icon container", n_ctx > 0 ? ctx_id[0] : 0, ID_ICONS );
    check( "... and One", n_ctx > 0 ? (LONG)ctx_rec[0] : 0, (LONG)icon_rec[0] );
    check( "Shift+F10 in the tree: the tree", n_ctx > 1 ? ctx_id[1] : 0, ID_TREE );
    check( "... and the cursored Beta", n_ctx > 1 ? (LONG)ctx_rec[1] : 0, (LONG)beta );
    check( "the wheel scrolled the tree", n_tree_scroll > 0, TRUE );
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
        case CN_CONTEXTMENU:
            if( n_ctx < MAX_LOG ) {
                ctx_id[n_ctx] = SHORT1FROMMP( mp1 );
                ctx_rec[n_ctx] = (PRECORDCORE)mp2;
                n_ctx++;
            }
            return 0;
        case CN_HELP:
            n_help++;
            help_rec = (PRECORDCORE)mp2;
            return 0;
        case CN_SCROLL:
            if( SHORT1FROMMP( mp1 ) == ID_TREE ) n_tree_scroll++;
            return 0;
        }
        return 0;
    case WM_CLOSE:
        final_checks();
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "CnrGridTest", ClientProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "CnrGridTest",
                                "Container trees and grids", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 500, 340, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    build_tree();
    build_icons();

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( frame );
    append_str( append_long( append_str( append_long( append_str( line, "pm_cnrgrid_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
