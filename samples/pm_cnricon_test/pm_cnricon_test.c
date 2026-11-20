/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_cnricon_test.c - WC_CONTAINER, stage 2b: icon view, icons, filtering
 * and direct editing
 *
 * A standard frame at desktop (100, 200), 400 x 300, holds in its client a
 * container at client (8, 8), 380 x 250, in the icon view. Six records
 * show system icons (file, folder, program, several files, application).
 * The program checks that
 *
 *   CM_FILTER calls the filter function for each record (with its
 *   pStorage) and marks the one it rejects ("Readme") CRA_FILTERED;
 *   CM_QUERYRECORD with CMA_FILTER passes over it;
 *   CM_ARRANGE places the shown records in a row from the upper left
 *   (writing ptlIcon) and leaves the filtered one where it was;
 *   CM_QUERYRECORDRECT (CMA_ICON) and CM_QUERYRECORDFROMRECT find the
 *   records placed at ptlIcon;
 *   CM_OPENEDIT sends CN_BEGINEDIT, and CM_CLOSEEDIT CN_REALLOCPSZ (refused
 *   here: the text stays) and CN_ENDEDIT.
 *
 * Then the records are placed in two rows: Folder (30, 150), Programs
 * (150, 150), Notes (270, 150), Docs (30, 50), App (150, 50). The
 * container's (0, 0) is screen (110, 559); the Folder's text is centred at
 * screen (157, 417), the Programs' at (277, 417). The integration test
 *
 *   Alt+clicks the Folder's text, types "Archive" and clicks white space
 *   (410, 329): the edit is kept (CN_REALLOCPSZ, the owner gives a new
 *   buffer, CN_ENDEDIT);
 *   Alt+clicks the Programs' text, types and presses Esc: discarded;
 *
 * and closes the window with Alt+F4, when the notifications, the texts and
 * a pel of the folder icon are checked. Each check is written to stdout:
 *
 *   pm_cnricon_test: <name>: ok
 *   pm_cnricon_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_cnricon_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINSTDCNR
#include <os2.h>
#include <string.h>

static HAB   hab;
static HWND  frame, client, cnr;
static PRECORDCORE rec[6];
static int   n_checks = 0;
static int   n_passed = 0;

/* The filter function: how often it ran, the pStorage it saw. */
static int   n_filtered = 0;
static PVOID seen_storage = NULL;
static ULONG storage_word = 0xA5A5;
/* Editing notifications. */
static int   n_begin = 0, n_realloc = 0, n_end = 0;
static ULONG begin_id = 0, last_cb = 0;
static PRECORDCORE begin_rec = NULL;
static BOOL  refuse = FALSE;
static char  newbuf[64];

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
    char *p = append_str( append_str( line, "pm_cnricon_test: " ), what );
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

static BOOL EXPENTRY Filter( PRECORDCORE p, PVOID pStorage )
{
    n_filtered++;
    seen_storage = pStorage;
    return strcmp( p->pszIcon, "Readme" ) != 0;
}

static void final_checks( void )
{
    HPS hps;

    check( "CN_BEGINEDIT for each edit", n_begin, 3 );
    check( "CN_REALLOCPSZ only for kept edits", n_realloc, 2 );
    check( "CN_ENDEDIT for each edit", n_end, 3 );
    check( "the folder's new text", strcmp( rec[1]->pszIcon, "Archive" ), 0 );
    check( "... in the owner's buffer", rec[1]->pszIcon == newbuf, TRUE );
    check( "... cbText counts the NUL", (LONG)last_cb, 8 );
    check( "the discarded edit left Programs", strcmp( rec[2]->pszIcon, "Programs" ), 0 );
    check( "edits are in the container's window", (LONG)begin_id, 1 );
    /* The folder pictogram's tan body, inside its icon at container
     * (31, 151); read through the client's PS (container (8, 8)), as
     * Warpine does not offset a child window's PS yet. */
    hps = WinGetPS( client );
    {
        POINTL pt = { 8 + 47, 8 + 164 };
        check( "the folder icon is drawn", GpiQueryPel( hps, &pt ), 0xE8C060 );
    }
    WinReleasePS( hps );
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
    case WM_CONTROL: {
        PCNREDITDATA ed = (PCNREDITDATA)mp2;
        switch( SHORT2FROMMP( mp1 ) ) {
        case CN_BEGINEDIT:
            n_begin++;
            begin_id = ed->id;
            begin_rec = ed->pRecord;
            return 0;
        case CN_REALLOCPSZ:
            n_realloc++;
            last_cb = ed->cbText;
            if( refuse || ed->cbText > sizeof( newbuf ) ) return (MRESULT)FALSE;
            *ed->ppszText = newbuf;
            return (MRESULT)TRUE;
        case CN_ENDEDIT:
            n_end++;
            return 0;
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

static PRECORDCORE one_record( PSZ text, ULONG sptr )
{
    RECORDINSERT ri;
    PRECORDCORE  r = (PRECORDCORE)WinSendMsg( cnr, CM_ALLOCRECORD, 0, MPFROMSHORT( 1 ) );
    r->pszIcon = r->pszText = r->pszName = r->pszTree = text;
    r->hptrIcon = WinQuerySysPointer( HWND_DESKTOP, sptr, FALSE );
    memset( &ri, 0, sizeof( ri ) );
    ri.cb = sizeof( ri );
    ri.pRecordOrder = (PRECORDCORE)CMA_END;
    ri.fInvalidateRecord = TRUE;
    ri.zOrder = CMA_TOP;
    ri.cRecordsInsert = 1;
    WinSendMsg( cnr, CM_INSERTRECORD, MPFROMP( r ), MPFROMP( &ri ) );
    return r;
}

static void place( PRECORDCORE r, LONG x, LONG y )
{
    r->ptlIcon.x = x;
    r->ptlIcon.y = y;
}

static void build_container( void )
{
    static const ULONG pos[6][2] = { { 270, 50 }, { 30, 150 }, { 150, 150 }, { 270, 150 }, { 30, 50 }, { 150, 50 } };
    CNRINFO          ci;
    QUERYRECORDRECT  qrr;
    QUERYRECFROMRECT qrfr;
    CNREDITDATA      ed;
    RECTL            r;
    int              i;

    cnr = WinCreateWindow( client, WC_CONTAINER, "", WS_VISIBLE,
                           8, 8, 380, 250, client, HWND_TOP, 1, NULL, NULL );
    check( "the container is created", cnr != NULLHANDLE, TRUE );
    memset( &ci, 0, sizeof( ci ) );
    ci.cb = sizeof( ci );
    ci.flWindowAttr = CV_ICON;
    WinSendMsg( cnr, CM_SETCNRINFO, MPFROMP( &ci ), MPFROMLONG( CMA_FLWINDOWATTR ) );
    rec[0] = one_record( "Readme", SPTR_FILE );
    rec[1] = one_record( "Folder", SPTR_FOLDER );
    rec[2] = one_record( "Programs", SPTR_PROGRAM );
    rec[3] = one_record( "Notes", SPTR_FILE );
    rec[4] = one_record( "Docs", SPTR_MULTFILE );
    rec[5] = one_record( "App", SPTR_APPICON );

    /* Filtering. */
    check( "CM_FILTER", (LONG)WinSendMsg( cnr, CM_FILTER, MPFROMP( Filter ), MPFROMP( &storage_word ) ), TRUE );
    check( "the filter ran for each record", n_filtered, 6 );
    check( "... with pStorage", seen_storage == &storage_word, TRUE );
    check( "Readme is filtered", ( rec[0]->flRecordAttr & CRA_FILTERED ) != 0, TRUE );
    check( "Folder is not", ( rec[1]->flRecordAttr & CRA_FILTERED ) != 0, FALSE );
    check( "CMA_FILTER passes over Readme",
           (PRECORDCORE)WinSendMsg( cnr, CM_QUERYRECORD, 0, MPFROM2SHORT( CMA_FIRST | CMA_FILTER, CMA_ITEMORDER ) ) == rec[1],
           TRUE );

    /* CM_ARRANGE: one row from the upper left of the 248-pel work area. */
    check( "CM_ARRANGE", (LONG)WinSendMsg( cnr, CM_ARRANGE, MPFROMLONG( CMA_ARRANGESTANDARD ), 0 ), TRUE );
    check( "the first shown record in the top row", rec[1]->ptlIcon.y, 248 - 8 - 32 );
    check( "the next one beside it", rec[2]->ptlIcon.y == rec[1]->ptlIcon.y && rec[2]->ptlIcon.x > rec[1]->ptlIcon.x, TRUE );
    check( "the filtered record stays where it was", rec[0]->ptlIcon.x == 0 && rec[0]->ptlIcon.y == 0, TRUE );

    /* Two rows of our own. */
    for( i = 0; i < 6; i++ ) place( rec[i], pos[i][0], pos[i][1] );
    WinSendMsg( cnr, CM_INVALIDATERECORD, 0, MPFROM2SHORT( 0, CMA_REPOSITION ) );
    memset( &qrr, 0, sizeof( qrr ) );
    qrr.cb = sizeof( qrr );
    qrr.pRecord = rec[1];
    qrr.fsExtent = CMA_ICON;
    check( "CM_QUERYRECORDRECT", (LONG)WinSendMsg( cnr, CM_QUERYRECORDRECT, MPFROMP( &r ), MPFROMP( &qrr ) ), TRUE );
    check( "... the folder's icon left", r.xLeft, 31 );
    check( "... bottom", r.yBottom, 151 );
    check( "... right", r.xRight, 63 );
    check( "... top", r.yTop, 183 );
    memset( &qrfr, 0, sizeof( qrfr ) );
    qrfr.cb = sizeof( qrfr );
    qrfr.rect.xLeft = 140; qrfr.rect.yBottom = 140; qrfr.rect.xRight = 200; qrfr.rect.yTop = 200;
    qrfr.fsSearch = CMA_PARTIAL | CMA_ITEMORDER;
    check( "CM_QUERYRECORDFROMRECT finds Programs",
           (PRECORDCORE)WinSendMsg( cnr, CM_QUERYRECORDFROMRECT, MPFROMLONG( CMA_FIRST ), MPFROMP( &qrfr ) ) == rec[2], TRUE );
    check( "... and nothing after it",
           (LONG)WinSendMsg( cnr, CM_QUERYRECORDFROMRECT, MPFROMP( rec[2] ), MPFROMP( &qrfr ) ), 0 );

    /* CM_OPENEDIT / CM_CLOSEEDIT, the new text refused. */
    memset( &ed, 0, sizeof( ed ) );
    ed.cb = sizeof( ed );
    ed.hwndCnr = cnr;
    ed.pRecord = rec[4];
    check( "CM_OPENEDIT", (LONG)WinSendMsg( cnr, CM_OPENEDIT, MPFROMP( &ed ), 0 ), TRUE );
    check( "... CN_BEGINEDIT already received", n_begin, 1 );
    check( "... for Docs", begin_rec == rec[4], TRUE );
    refuse = TRUE;
    check( "CM_CLOSEEDIT", (LONG)WinSendMsg( cnr, CM_CLOSEEDIT, 0, 0 ), TRUE );
    refuse = FALSE;
    check( "... CN_REALLOCPSZ", n_realloc, 1 );
    check( "... CN_ENDEDIT", n_end, 1 );
    check( "... the refused text stays", strcmp( rec[4]->pszIcon, "Docs" ), 0 );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "CnrIconTest", ClientProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "CnrIconTest",
                                "Container icons", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    build_container();

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( frame );
    append_str( append_long( append_str( append_long( append_str( line, "pm_cnricon_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
