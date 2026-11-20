/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_notebook_test.c - the WC_NOTEBOOK control
 *
 * A standard frame (title bar, sizing border) at desktop (100, 200),
 * 400 x 300, holds in its client a notebook at client (8, 8), 380 x 250,
 * major tabs on the right. Its pages, each a dialog loaded with the
 * notebook as parent (embedded in it, ADR-0042) and sized to the page
 * (BKA_AUTOPAGESIZE):
 *
 *   P1  major tab "First", status line "Page 1 of 3", an entry field
 *   P2  minor tab "Sub" in P1's section, a check box
 *   P3  major tab "Second", a text
 *
 * The program checks BKM_INSERTPAGE / QUERYPAGEID / QUERYPAGECOUNT /
 * SETTABTEXT / QUERYTABTEXT / SETSTATUSLINETEXT / QUERYPAGESTYLE /
 * SETPAGEDATA / QUERYPAGEDATA / SETPAGEWINDOWHWND / QUERYPAGEWINDOWHWND /
 * CALCPAGERECT, that only the top page's window is shown and sized to the
 * page, and that BKM_TURNTOPAGE sends BKN_PAGESELECTED (a PAGESELECTNOTIFY)
 * before it returns; it ends on P3. The integration test (the client's
 * (0, 0) is screen (102, 567), screen y = 767 - PM y) then clicks:
 *
 *   the "First" tab (screen 446, 325) -> P1;
 *   the "Sub" minor tab (144, 545)    -> P2;
 *   the "First" tab again             -> P1;
 *   the entry field on P1 (175, 422) and types "xyz";
 *
 * and closes the window with Alt+F4: the notifications, the visible page
 * and the typed text are checked, then BKM_DELETEPAGE sends
 * BKN_PAGEDELETED (a DELETENOTIFY). Each check is written to stdout:
 *
 *   pm_notebook_test: <name>: ok
 *   pm_notebook_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_notebook_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#include <os2.h>
#include <string.h>
#include "pm_notebook_test.h"

#define MAX_LOG 16

static HAB   hab;
static HWND  frame, client, book, page[3];
static ULONG id[3];
static int   n_checks = 0;
static int   n_passed = 0;

/* BKN_PAGESELECTED: (current, new) page ids as the owner read them. */
static ULONG sel_cur[MAX_LOG], sel_new[MAX_LOG];
static int   n_sel = 0;
/* BKN_PAGEDELETED: the page window and data. */
static HWND  del_hwnd = 0;
static ULONG del_data = 0;
static int   n_del = 0;

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
    char *p = append_str( append_str( line, "pm_notebook_test: " ), what );
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

static MRESULT EXPENTRY PageProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_COMMAND ) return 0;   /* keep the page (no dismissal) */
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

static void selected_was( int n, int from, int to, const char *what )
{
    char name[64];
    append_str( append_str( name, what ), ": from" );
    check( name, n < n_sel ? (LONG)sel_cur[n] : -1, (LONG)id[from] );
    append_str( append_str( name, what ), ": to" );
    check( name, n < n_sel ? (LONG)sel_new[n] : -1, (LONG)id[to] );
}

static void final_checks( void )
{
    char text[32];
    HWND entry = WinWindowFromID( page[0], ID_NAME );

    check( "notifications from the clicks", n_sel, 4 );
    selected_was( 1, 2, 0, "click First" );
    selected_was( 2, 0, 1, "click Sub" );
    selected_was( 3, 1, 0, "click First again" );
    check( "top page after the clicks", (LONG)WinSendMsg( book, BKM_QUERYPAGEID, 0, MPFROM2SHORT( BKA_TOP, 0 ) ), id[0] );
    check( "page 1 shown", WinIsWindowVisible( page[0] ), TRUE );
    check( "page 2 hidden", WinIsWindowVisible( page[1] ), FALSE );
    check( "the entry field has the focus", WinQueryFocus( HWND_DESKTOP ) == entry, TRUE );
    WinQueryWindowText( entry, sizeof( text ), text );
    check( "the typed text", strcmp( text, "xyz" ) == 0, TRUE );
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
        if( SHORT2FROMMP( mp1 ) == BKN_PAGESELECTED && n_sel < MAX_LOG ) {
            PPAGESELECTNOTIFY psn = (PPAGESELECTNOTIFY)mp2;
            sel_cur[n_sel] = psn->ulPageIdCur;
            sel_new[n_sel] = psn->ulPageIdNew;
            n_sel++;
        } else if( SHORT2FROMMP( mp1 ) == BKN_PAGEDELETED ) {
            PDELETENOTIFY pdn = (PDELETENOTIFY)mp2;
            del_hwnd = pdn->hwndPage;
            del_data = pdn->ulAppPageData;
            n_del++;
        }
        return 0;
    case WM_CLOSE:
        final_checks();
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static void build_book( void )
{
    static const char *tabs[3] = { "First", "Sub", "Second" };
    static const USHORT styles[3] = { BKA_MAJOR | BKA_STATUSTEXTON | BKA_AUTOPAGESIZE,
                                      BKA_MINOR | BKA_AUTOPAGESIZE, BKA_MAJOR | BKA_AUTOPAGESIZE };
    static const ULONG dlgs[3] = { ID_PAGE1, ID_PAGE2, ID_PAGE3 };
    char    text[32];
    BOOKTEXT bt;
    RECTL   r, r0;
    SWP     swp;
    int     i;

    book = WinCreateWindow( client, WC_NOTEBOOK, "", WS_VISIBLE | BKS_MAJORTABRIGHT | BKS_BACKPAGESBR,
                            8, 8, 380, 250, client, HWND_TOP, 1, NULL, NULL );
    check( "the notebook is created", book != NULLHANDLE, TRUE );
    for( i = 0; i < 3; i++ ) {
        id[i] = (ULONG)WinSendMsg( book, BKM_INSERTPAGE, 0, MPFROM2SHORT( styles[i], BKA_LAST ) );
        WinSendMsg( book, BKM_SETTABTEXT, MPFROMLONG( id[i] ), MPFROMP( tabs[i] ) );
        WinSendMsg( book, BKM_SETPAGEDATA, MPFROMLONG( id[i] ), MPFROMLONG( 100 + i ) );
        page[i] = WinLoadDlg( book, book, PageProc, NULLHANDLE, dlgs[i], NULL );
        WinSendMsg( book, BKM_SETPAGEWINDOWHWND, MPFROMLONG( id[i] ), MPFROMHWND( page[i] ) );
    }
    WinSendMsg( book, BKM_SETSTATUSLINETEXT, MPFROMLONG( id[0] ), MPFROMP( "Page 1 of 3" ) );

    check( "page ids", id[0] != 0 && id[1] != 0 && id[2] != 0 && id[0] != id[1] && id[1] != id[2], TRUE );
    check( "page count", (LONG)WinSendMsg( book, BKM_QUERYPAGECOUNT, 0, 0 ), 3 );
    check( "first page", (LONG)WinSendMsg( book, BKM_QUERYPAGEID, 0, MPFROM2SHORT( BKA_FIRST, 0 ) ), id[0] );
    check( "next page", (LONG)WinSendMsg( book, BKM_QUERYPAGEID, MPFROMLONG( id[0] ), MPFROM2SHORT( BKA_NEXT, 0 ) ), id[1] );
    check( "next major page", (LONG)WinSendMsg( book, BKM_QUERYPAGEID, MPFROMLONG( id[0] ),
                                                MPFROM2SHORT( BKA_NEXT, BKA_MAJOR ) ), id[2] );
    check( "top page", (LONG)WinSendMsg( book, BKM_QUERYPAGEID, 0, MPFROM2SHORT( BKA_TOP, 0 ) ), id[0] );
    check( "page style", (LONG)WinSendMsg( book, BKM_QUERYPAGESTYLE, MPFROMLONG( id[1] ), 0 ),
           BKA_MINOR | BKA_AUTOPAGESIZE );
    check( "page data", (LONG)WinSendMsg( book, BKM_QUERYPAGEDATA, MPFROMLONG( id[2] ), 0 ), 102 );
    check( "page window", (LONG)WinSendMsg( book, BKM_QUERYPAGEWINDOWHWND, MPFROMLONG( id[1] ), 0 ), (LONG)page[1] );
    bt.pString = text; bt.textLen = sizeof( text );
    check( "tab text length", (LONG)WinSendMsg( book, BKM_QUERYTABTEXT, MPFROMLONG( id[2] ), MPFROMP( &bt ) ), 6 );
    check( "tab text", strcmp( text, "Second" ) == 0, TRUE );
    check( "a bad page id", (LONG)WinSendMsg( book, BKM_QUERYPAGEDATA, MPFROMLONG( 12345 ), 0 ), BOOKERR_INVALID_PARAMETERS );

    check( "the top page's window is shown", WinIsWindowVisible( page[0] ), TRUE );
    check( "the others are hidden", WinIsWindowVisible( page[1] ) || WinIsWindowVisible( page[2] ), FALSE );
    WinQueryWindowPos( page[0], &swp );
    check( "the page window fills the page (inside the notebook)",
           swp.x > 0 && swp.y > 0 && swp.x + swp.cx < 380 && swp.y + swp.cy < 250 && swp.cx > 200, TRUE );
    /* BKM_CALCPAGERECT there and back. */
    r.xLeft = 8; r.yBottom = 8; r.xRight = 388; r.yTop = 258;
    r0 = r;
    WinSendMsg( book, BKM_CALCPAGERECT, MPFROMP( &r ), MPFROMLONG( TRUE ) );
    check( "page rectangle inside the notebook", r.xLeft > r0.xLeft && r.yTop < r0.yTop, TRUE );
    check( "page rectangle is the page window's", r.xRight - r.xLeft, swp.cx );
    WinSendMsg( book, BKM_CALCPAGERECT, MPFROMP( &r ), MPFROMLONG( FALSE ) );
    check( "and back to the notebook's", r.xLeft == r0.xLeft && r.yTop == r0.yTop, TRUE );

    /* Turning sends BKN_PAGESELECTED before BKM_TURNTOPAGE returns. */
    check( "BKM_TURNTOPAGE", (LONG)WinSendMsg( book, BKM_TURNTOPAGE, MPFROMLONG( id[2] ), 0 ), TRUE );
    check( "BKN_PAGESELECTED already received", n_sel, 1 );
    selected_was( 0, 0, 2, "turn to page 3" );
    check( "page 3 shown", WinIsWindowVisible( page[2] ), TRUE );
    check( "page 1 hidden", WinIsWindowVisible( page[0] ), FALSE );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "NotebookTest", ClientProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "NotebookTest",
                                "Notebook", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    build_book();

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    /* Deleting a page sends BKN_PAGEDELETED. */
    check( "BKM_DELETEPAGE", (LONG)WinSendMsg( book, BKM_DELETEPAGE, MPFROMLONG( id[1] ), MPFROMSHORT( BKA_SINGLE ) ), TRUE );
    check( "BKN_PAGEDELETED received", n_del, 1 );
    check( "... with the page window", del_hwnd == page[1], TRUE );
    check( "... and data", (LONG)del_data, 101 );
    check( "page count after the deletion", (LONG)WinSendMsg( book, BKM_QUERYPAGECOUNT, 0, 0 ), 2 );

    WinDestroyWindow( frame );
    append_str( append_long( append_str( append_long( append_str( line, "pm_notebook_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
