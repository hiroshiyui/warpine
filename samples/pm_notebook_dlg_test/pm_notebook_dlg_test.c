/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_notebook_dlg_test.c - a tabbed-dialog notebook in a dialog
 *
 * WinDlgBox shows "Settings" (ID_MAIN), which holds a WC_NOTEBOOK of style
 * BKS_TABBEDDIALOG (from its dialog template) and a "Close" push button.
 * In WM_INITDLG the program gives the notebook six pages, each set up with
 * BKM_SETPAGEINFO:
 *
 *   P1 "General"  dialog resource ID_PAGE1, loaded at once (bLoadDlg TRUE)
 *   P2 "Colour"   dialog resource ID_PAGE2, loaded when first turned to,
 *                 a tab colour (BKM_SETTABCOLOR)
 *   P3 "Locked"   the owner refuses to turn to it (BKN_PAGESELECTEDPENDING)
 *   P4 "Memory"   the DLGTEMPLATE ID_PAGE4 in memory (DosGetResource),
 *                 loaded when first turned to
 *   P5 "Picture"  a tab bitmap (BFA_MAJORTABBITMAP), no window
 *   P6 "Last"     no window
 *
 * and two notebook buttons (BKM_SETNOTEBOOKBUTTONS): "Undo" (with the red
 * picture as its hImage) and
 * "Default". Six tabs do not fit the notebook's width (three do), so it
 * shows scroll arrows. The program checks what it can at once; the integration test
 * then (see tests/integration.rs, NOTEBOOK_DLG_STEPS):
 *
 *   clicks Undo                  -> WM_COMMAND(ID_UNDO) to P1's window;
 *   clicks the "Colour" tab      -> P2 is loaded and turned to;
 *   clicks Default               -> WM_COMMAND(ID_DEFAULT) to P2's window;
 *   clicks the "Locked" tab      -> refused, P2 stays on top;
 *   clicks the right scroll arrow twice, then the third tab shown -> P5;
 *   clicks the "Memory" tab      -> P4 is loaded from memory;
 *   presses Home                 -> P1 (its tab scrolls back into view);
 *   presses Tab                  -> the focus moves into P1's entry field;
 *   types "abc", presses Tab     -> the focus leaves the page for Close;
 *   presses Enter                -> Close: the final checks.
 *
 * Each check is written to stdout:
 *
 *   pm_notebook_dlg_test: <name>: ok
 *   pm_notebook_dlg_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_notebook_dlg_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOS
#include <os2.h>
#include <string.h>
#include "pm_notebook_dlg_test.h"

#define MAX_LOG  16
#define PARAMS   0x4D454D21UL   /* pPageDlgCreateParams of P2 */

static HAB   hab;
static HWND  book;
static ULONG id[6];
static int   n_checks = 0;
static int   n_passed = 0;

/* BKN_PAGESELECTEDPENDING / BKN_PAGESELECTED: the new page ids. */
static ULONG pending[MAX_LOG], selected[MAX_LOG];
static int   n_pending = 0, n_selected = 0;
/* What the page windows received. */
static ULONG p1_command = 0, p2_command = 0, p2_initdlg = 0;
static int   name_focus = 0;
static HBITMAP hbm_tab = NULLHANDLE;

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
    char *p = append_str( append_str( line, "pm_notebook_dlg_test: " ), what );
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

static int logged( ULONG *log, int n, ULONG page )
{
    int i;
    for( i = 0; i < n; i++ ) if( log[i] == page ) return 1;
    return 0;
}

static HWND page_window( int i )
{
    return (HWND)WinSendMsg( book, BKM_QUERYPAGEWINDOWHWND, MPFROMLONG( id[i] ), 0 );
}

static ULONG top_page( void )
{
    return (ULONG)WinSendMsg( book, BKM_QUERYPAGEID, 0, MPFROM2SHORT( BKA_TOP, 0 ) );
}

/* The page dialogs: keep the page on WM_COMMAND, note what arrives. */
static MRESULT EXPENTRY PageProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_INITDLG:
        if( WinQueryWindowUShort( hwnd, QWS_ID ) == ID_PAGE2 ) p2_initdlg = (ULONG)mp2;
        break;
    case WM_COMMAND:
        if( WinQueryWindowUShort( hwnd, QWS_ID ) == ID_PAGE1 ) p1_command = SHORT1FROMMP( mp1 );
        if( WinQueryWindowUShort( hwnd, QWS_ID ) == ID_PAGE2 ) p2_command = SHORT1FROMMP( mp1 );
        return 0;
    case WM_CONTROL:
        if( SHORT1FROMMP( mp1 ) == ID_NAME && SHORT2FROMMP( mp1 ) == EN_SETFOCUS ) name_focus++;
        return 0;
    }
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

/* A red 8 x 8 bitmap for P5's tab. */
static HBITMAP red_bitmap( void )
{
    static BYTE bits[8 * 24];
    BITMAPINFOHEADER2 bmih;
    BITMAPINFO2       info;
    SIZEL sizl = { 0, 0 };
    HDC   hdc = DevOpenDC( hab, OD_MEMORY, "*", 0L, NULL, NULLHANDLE );
    HPS   hps = GpiCreatePS( hab, hdc, &sizl, PU_PELS | GPIT_MICRO | GPIA_ASSOC );
    int   i;

    for( i = 0; i < (int)sizeof( bits ); i += 3 ) {
        bits[i] = 0; bits[i + 1] = 0; bits[i + 2] = 0xFF;      /* B, G, R */
    }
    memset( &bmih, 0, sizeof( bmih ) );
    bmih.cbFix = sizeof( bmih );
    bmih.cx = 8; bmih.cy = 8; bmih.cPlanes = 1; bmih.cBitCount = 24;
    memset( &info, 0, sizeof( info ) );
    info.cbFix = 16;
    info.cx = 8; info.cy = 8; info.cPlanes = 1; info.cBitCount = 24;
    return GpiCreateBitmap( hps, &bmih, CBM_INIT, bits, &info );
}

static void set_page( int i, ULONG fl, BOOL load, ULONG dlg, PDLGTEMPLATE tmpl, ULONG params, const char *tab )
{
    BOOKPAGEINFO bpi;
    memset( &bpi, 0, sizeof( bpi ) );
    bpi.cb = sizeof( bpi );
    bpi.fl = fl | BFA_MAJORTABTEXT;
    bpi.bLoadDlg = load;
    bpi.pfnPageDlgProc = (PFN)PageProc;
    bpi.idPageDlg = dlg;
    bpi.hmodPageDlg = NULLHANDLE;
    bpi.pPageDlgCreateParams = (PVOID)params;
    bpi.pdlgtPage = tmpl;
    bpi.cbMajorTab = strlen( tab );
    bpi.pszMajorTab = (PSZ)tab;
    bpi.hbmMajorTab = hbm_tab;
    check( tab, (LONG)WinSendMsg( book, BKM_SETPAGEINFO, MPFROMLONG( id[i] ), MPFROMP( &bpi ) ), TRUE );
}

static void build_book( HWND dlg )
{
    static NOTEBOOKBUTTON buttons[2] = {
        { "Undo", ID_UNDO, NULLHANDLE, 0 },
        { "Default", ID_DEFAULT, NULLHANDLE, 0 },
    };
    NOTEBOOKBUTTON bad = { "Bad", BKA_MAXBUTTONID + 1, NULLHANDLE, 0 };
    PVOID tmpl = NULL;
    HWND  undo;
    int   i;

    book = WinWindowFromID( dlg, ID_BOOK );
    check( "the notebook from the template", book != NULLHANDLE, TRUE );
    for( i = 0; i < 6; i++ ) {
        id[i] = (ULONG)WinSendMsg( book, BKM_INSERTPAGE, 0,
                                   MPFROM2SHORT( BKA_MAJOR | BKA_AUTOPAGESIZE, BKA_LAST ) );
    }
    check( "DosGetResource of the page template",
           DosGetResource( NULLHANDLE, RT_DIALOG, ID_PAGE4, &tmpl ), 0 );
    hbm_tab = red_bitmap();
    check( "a bitmap for a tab", hbm_tab != NULLHANDLE, TRUE );

    set_page( 0, BFA_PAGEFROMDLGRES, TRUE, ID_PAGE1, NULL, 0, "General" );
    set_page( 1, BFA_PAGEFROMDLGRES, FALSE, ID_PAGE2, NULL, PARAMS, "Colour" );
    set_page( 2, 0, FALSE, 0, NULL, 0, "Locked" );
    set_page( 3, BFA_PAGEFROMDLGTEMPLATE, FALSE, 0, (PDLGTEMPLATE)tmpl, 0, "Memory" );
    set_page( 4, BFA_MAJORTABBITMAP, FALSE, 0, NULL, 0, "Picture" );
    set_page( 5, 0, FALSE, 0, NULL, 0, "Last" );

    check( "P1 loaded at once", page_window( 0 ) != NULLHANDLE, TRUE );
    check( "... as a child of the notebook", WinQueryWindow( page_window( 0 ), QW_PARENT ) == book, TRUE );
    check( "... and shown", WinIsWindowVisible( page_window( 0 ) ), TRUE );
    check( "P2 not loaded before it is turned to", page_window( 1 ) == NULLHANDLE, TRUE );
    check( "P5's tab bitmap", (LONG)WinSendMsg( book, BKM_QUERYTABBITMAP, MPFROMLONG( id[4] ), 0 ), (LONG)hbm_tab );
    check( "BKM_SETTABCOLOR", (LONG)WinSendMsg( book, BKM_SETTABCOLOR, MPFROMLONG( id[1] ),
                                                MPFROMLONG( 0x00C0E0FF ) ), TRUE );
    check( "BKM_SETTABCOLOR of a bad page", (LONG)WinSendMsg( book, BKM_SETTABCOLOR, MPFROMLONG( 12345 ),
                                                              MPFROMLONG( 0 ) ), FALSE );

    check( "BKM_SETNOTEBOOKBUTTONS with a bad id",
           (LONG)WinSendMsg( book, BKM_SETNOTEBOOKBUTTONS, MPFROMLONG( 1 ), MPFROMP( &bad ) ), FALSE );
    buttons[0].hImage = hbm_tab;     /* "Undo" shows the red picture */
    check( "BKM_SETNOTEBOOKBUTTONS",
           (LONG)WinSendMsg( book, BKM_SETNOTEBOOKBUTTONS, MPFROMLONG( 2 ), MPFROMP( buttons ) ), TRUE );
    undo = WinWindowFromID( book, ID_UNDO );
    check( "the Undo button is the notebook's child", undo != NULLHANDLE, TRUE );
    check( "... a BS_NOTEBOOKBUTTON",
           (LONG)( WinQueryWindowULong( undo, QWL_STYLE ) & BS_PRIMARYSTYLES ), BS_NOTEBOOKBUTTON );
    check( "... with its id", WinQueryWindowUShort( undo, QWS_ID ), ID_UNDO );
}

static void final_checks( HWND dlg )
{
    char text[32];
    HWND entry = WinWindowFromID( page_window( 0 ), ID_NAME );

    check( "Undo reached P1", (LONG)p1_command, ID_UNDO );
    check( "Default reached P2", (LONG)p2_command, ID_DEFAULT );
    check( "P2 loaded when turned to", WinQueryWindow( page_window( 1 ), QW_PARENT ) == book, TRUE );
    check( "P2's WM_INITDLG creation parameters", (LONG)p2_initdlg, (LONG)PARAMS );
    check( "P2 has its check box", WinWindowFromID( page_window( 1 ), ID_OPTION ) != NULLHANDLE, TRUE );
    check( "P3 asked about", logged( pending, n_pending, id[2] ), TRUE );
    check( "P3 never selected", logged( selected, n_selected, id[2] ), FALSE );
    check( "the arrow scrolled the tabs: P5 selected", logged( selected, n_selected, id[4] ), TRUE );
    check( "P4 loaded from memory", WinWindowFromID( page_window( 3 ), ID_MEMLABEL ) != NULLHANDLE, TRUE );
    check( "every turn asked first", n_pending >= n_selected + 1, TRUE );
    check( "back on P1", (LONG)top_page(), (LONG)id[0] );
    check( "Tab moved into the page", name_focus, 1 );
    WinQueryWindowText( entry, sizeof( text ), text );
    check( "the typed text", strcmp( text, "abc" ) == 0, TRUE );
    check( "Tab left the page for Close", WinQueryFocus( HWND_DESKTOP ) == WinWindowFromID( dlg, DID_OK ), TRUE );
}

static MRESULT EXPENTRY MainProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_INITDLG:
        build_book( hwnd );
        return 0;
    case WM_CONTROL:
        if( SHORT1FROMMP( mp1 ) == ID_BOOK ) {
            PPAGESELECTNOTIFY psn = (PPAGESELECTNOTIFY)mp2;
            if( SHORT2FROMMP( mp1 ) == BKN_PAGESELECTEDPENDING && n_pending < MAX_LOG ) {
                pending[n_pending++] = psn->ulPageIdNew;
                if( psn->ulPageIdNew == id[2] ) psn->ulPageIdNew = 0;   /* refuse P3 */
            } else if( SHORT2FROMMP( mp1 ) == BKN_PAGESELECTED && n_selected < MAX_LOG ) {
                selected[n_selected++] = psn->ulPageIdNew;
            }
        }
        return 0;
    case WM_COMMAND:
        if( SHORT1FROMMP( mp1 ) == DID_OK ) {
            final_checks( hwnd );
            WinDismissDlg( hwnd, DID_OK );
        }
        return 0;
    }
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ  hmq;
    char line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinDlgBox( HWND_DESKTOP, HWND_DESKTOP, MainProc, NULLHANDLE, ID_MAIN, NULL );
    append_str( append_long( append_str( append_long( append_str( line, "pm_notebook_dlg_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
