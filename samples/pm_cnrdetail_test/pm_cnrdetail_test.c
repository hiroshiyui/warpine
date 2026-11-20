/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_cnrdetail_test.c - the WC_CONTAINER details view, second stage
 *
 * A standard frame at desktop (100, 200), 400 x 300, holds in its client a
 * container at client (8, 8), 380 x 250, in the details view with a title
 * ("Files") and column headings. Its four columns are wider than the
 * container:
 *
 *   Name  (CFA_STRING, 100 pels)  left of the split bar (pFieldInfoLast)
 *   Size  (CFA_ULONG,   80 pels)  right of it ...
 *   Kind  (CFA_STRING | CFA_OWNER, 120 pels): drawn by the owner
 *   Note  (CFA_STRING, 200 pels)
 *
 * with the split bar at 80 pels (xVertSplitbar). The owner draws the Kind
 * cells (WM_DRAWITEM with a CNRDRAWITEMINFO) as coloured bars, except
 * beta's, for which it answers FALSE so that the container draws the
 * text. The program checks CM_QUERYCNRINFO, CM_QUERYVIEWPORTRECT of both
 * panes, CM_SCROLLWINDOW across the left pane (CN_SCROLL with
 * CMA_HORIZONTAL before it returns), the scroll bar windows, and a title
 * edit opened and closed by messages. The integration test (see
 * tests/integration.rs, CNRDETAIL_STEPS) then:
 *
 *   drags the split bar 40 pels to the right;
 *   clicks the right pane's scroll bar arrow;
 *   Alt+clicks the title, types "Report", clicks a row  -> the new title;
 *   Alt+clicks the Note heading, types "Remark", clicks a row;
 *
 * and closes the window with Alt+F4: the split bar's place, the CN_SCROLL
 * from the arrow and the edited title and heading are checked. Each check
 * is written to stdout:
 *
 *   pm_cnrdetail_test: <name>: ok
 *   pm_cnrdetail_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_cnrdetail_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINSTDCNR
#include <os2.h>
#include <string.h>

typedef struct _USERRECORD {
    RECORDCORE core;
    ULONG      size;
    PSZ        kind;
    PSZ        note;
} USERRECORD, *PUSERRECORD;

static HAB   hab;
static HWND  frame, client, cnr;
static PUSERRECORD rec[4];
static PFIELDINFO  fname, fsize, fkind, fnote;
static int   n_checks = 0;
static int   n_passed = 0;

/* WM_DRAWITEM: how many, whether every one named a Kind cell of a record. */
static int   n_draw = 0, draw_ok = 1, n_false = 0;
/* CN_SCROLL across, and the last increment. */
static int   n_hscroll = 0;
static LONG  last_inc = 0;
/* Direct edits: the new title and heading buffers, CN_ENDEDIT count. */
static char  title_buf[64] = "Files";
static char  head_buf[64] = "Note";
static int   n_endedit = 0, n_begin_title = 0;

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
    char *p = append_str( append_str( line, "pm_cnrdetail_test: " ), what );
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

static void final_checks( void )
{
    CNRINFO ci;
    memset( &ci, 0, sizeof( ci ) );
    WinSendMsg( cnr, CM_QUERYCNRINFO, MPFROMP( &ci ), MPFROMLONG( sizeof( ci ) ) );
    check( "the split bar was dragged to 120", ci.xVertSplitbar, 120 );
    check( "the arrow scrolled the right pane (CN_SCROLL)", n_hscroll >= 3, TRUE );
    check( "... to the right", last_inc > 0, TRUE );
    check( "the title was edited", ci.pszCnrTitle != NULL && strcmp( ci.pszCnrTitle, "Report" ) == 0, TRUE );
    check( "the heading was edited", fnote->pTitleData != NULL && strcmp( (char *)fnote->pTitleData, "Remark" ) == 0, TRUE );
    check( "CN_ENDEDIT for each edit", n_endedit, 3 );
    check( "the owner drew Kind cells", n_draw >= 3, TRUE );
    check( "... each a Kind cell of a record", draw_ok, TRUE );
    check( "... and left beta's to the container", n_false >= 1, TRUE );
}

/* WM_DRAWITEM: the Kind cells, as bars of a colour per record. */
static MRESULT draw_item( POWNERITEM oi )
{
    PCNRDRAWITEMINFO di = (PCNRDRAWITEMINFO)oi->hItem;
    PUSERRECORD r = di != NULL ? (PUSERRECORD)di->pRecord : NULL;
    RECTL rc = oi->rclItem;
    static const LONG colors[4] = { CLR_RED, CLR_GREEN, CLR_BLUE, CLR_YELLOW };
    int i;

    n_draw++;
    if( di == NULL || di->pFieldInfo != fkind || r == NULL ) {
        draw_ok = 0;
        return (MRESULT)FALSE;
    }
    for( i = 0; i < 4 && rec[i] != r; i++ ) ;
    if( i == 4 ) { draw_ok = 0; return (MRESULT)FALSE; }
    if( i == 1 ) { n_false++; return (MRESULT)FALSE; }  /* beta: the container draws it */
    rc.xLeft += 4; rc.xRight -= 4; rc.yBottom += 3; rc.yTop -= 3;
    WinFillRect( oi->hps, &rc, colors[i] );
    return (MRESULT)TRUE;
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
    case WM_DRAWITEM:
        return draw_item( (POWNERITEM)mp2 );
    case WM_CONTROL:
        switch( SHORT2FROMMP( mp1 ) ) {
        case CN_SCROLL: {
            PNOTIFYSCROLL ns = (PNOTIFYSCROLL)mp2;
            if( ns->fScroll & CMA_HORIZONTAL ) {
                n_hscroll++;
                last_inc = ns->lScrollInc;
            }
            return 0;
        }
        case CN_BEGINEDIT: {
            PCNREDITDATA ed = (PCNREDITDATA)mp2;
            if( ed->pRecord == NULL && ed->id == CID_CNRTITLEWND ) n_begin_title++;
            return 0;
        }
        case CN_REALLOCPSZ: {
            /* Give the title and the heading buffers of their own. */
            PCNREDITDATA ed = (PCNREDITDATA)mp2;
            if( ed->cbText > sizeof( title_buf ) ) return (MRESULT)FALSE;
            if( ed->pRecord == NULL && ed->id == CID_CNRTITLEWND ) {
                *ed->ppszText = title_buf;
                return (MRESULT)TRUE;
            }
            if( ed->pRecord == NULL && ed->pFieldInfo == fnote ) {
                *ed->ppszText = head_buf;
                return (MRESULT)TRUE;
            }
            return (MRESULT)FALSE;
        }
        case CN_ENDEDIT:
            n_endedit++;
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

static PFIELDINFO field( PFIELDINFO f, ULONG fl, PSZ title, ULONG off, ULONG width )
{
    f->flData = fl | CFA_SEPARATOR | CFA_HORZSEPARATOR;
    f->flTitle = CFA_LEFT;
    f->pTitleData = title;
    f->offStruct = off;
    f->cxWidth = width;
    return f->pNextFieldInfo;
}

static void build_container( void )
{
    static PSZ names[4] = { "alpha", "beta", "gamma", "delta" };
    static PSZ kinds[4] = { "text", "source", "header", "binary" };
    static PSZ notes[4] = { "a note on alpha", "a note on beta", "a note on gamma", "a note on delta" };
    CNRINFO         ci;
    FIELDINFOINSERT fii;
    RECORDINSERT    ri;
    CNREDITDATA     ed;
    PFIELDINFO      f;
    PUSERRECORD     r;
    RECTL           left, right;
    int             i;

    cnr = WinCreateWindow( client, WC_CONTAINER, "", WS_VISIBLE | CCS_SINGLESEL,
                           8, 8, 380, 250, client, HWND_TOP, 1, NULL, NULL );
    check( "the container is created", cnr != NULLHANDLE, TRUE );

    r = (PUSERRECORD)WinSendMsg( cnr, CM_ALLOCRECORD,
                                 MPFROMLONG( sizeof( USERRECORD ) - sizeof( RECORDCORE ) ), MPFROMSHORT( 4 ) );
    for( i = 0; i < 4; i++ ) {
        rec[i] = r;
        r->core.pszIcon = r->core.pszText = r->core.pszName = r->core.pszTree = names[i];
        r->size = 1000 * ( i + 1 );
        r->kind = kinds[i];
        r->note = notes[i];
        r = (PUSERRECORD)r->core.preccNextRecord;
    }
    memset( &ri, 0, sizeof( ri ) );
    ri.cb = sizeof( ri );
    ri.pRecordOrder = (PRECORDCORE)CMA_END;
    ri.fInvalidateRecord = TRUE;
    ri.zOrder = CMA_TOP;
    ri.cRecordsInsert = 4;
    check( "CM_INSERTRECORD", (LONG)WinSendMsg( cnr, CM_INSERTRECORD, MPFROMP( rec[0] ), MPFROMP( &ri ) ), 4 );

    fname = (PFIELDINFO)WinSendMsg( cnr, CM_ALLOCDETAILFIELDINFO, MPFROMSHORT( 4 ), 0 );
    f = field( fname, CFA_STRING, "Name", FIELDOFFSET( USERRECORD, core.pszName ), 100 );
    fsize = f;
    f = field( f, CFA_ULONG | CFA_RIGHT, "Size", FIELDOFFSET( USERRECORD, size ), 80 );
    fkind = f;
    f = field( f, CFA_STRING | CFA_OWNER, "Kind", FIELDOFFSET( USERRECORD, kind ), 120 );
    fnote = f;
    field( f, CFA_STRING, head_buf, FIELDOFFSET( USERRECORD, note ), 200 );
    memset( &fii, 0, sizeof( fii ) );
    fii.cb = sizeof( fii );
    fii.pFieldInfoOrder = (PFIELDINFO)CMA_END;
    fii.fInvalidateFieldInfo = TRUE;
    fii.cFieldInfoInsert = 4;
    check( "CM_INSERTDETAILFIELDINFO", (LONG)WinSendMsg( cnr, CM_INSERTDETAILFIELDINFO, MPFROMP( fname ), MPFROMP( &fii ) ), 4 );

    memset( &ci, 0, sizeof( ci ) );
    ci.cb = sizeof( ci );
    ci.flWindowAttr = CV_DETAIL | CA_DETAILSVIEWTITLES | CA_CONTAINERTITLE | CA_TITLESEPARATOR | CA_TITLELEFT;
    ci.pszCnrTitle = title_buf;
    ci.pFieldInfoLast = fname;
    ci.xVertSplitbar = 80;
    check( "CM_SETCNRINFO", (LONG)WinSendMsg( cnr, CM_SETCNRINFO, MPFROMP( &ci ),
           MPFROMLONG( CMA_FLWINDOWATTR | CMA_CNRTITLE | CMA_PFIELDINFOLAST | CMA_XVERTSPLITBAR ) ), TRUE );
    memset( &ci, 0, sizeof( ci ) );
    WinSendMsg( cnr, CM_QUERYCNRINFO, MPFROMP( &ci ), MPFROMLONG( sizeof( ci ) ) );
    check( "pFieldInfoLast", ci.pFieldInfoLast == fname, TRUE );
    check( "xVertSplitbar", ci.xVertSplitbar, 80 );

    /* The two panes: the left one 80 pels, the right one after the bar. */
    check( "CM_QUERYVIEWPORTRECT, left pane", (LONG)WinSendMsg( cnr, CM_QUERYVIEWPORTRECT, MPFROMP( &left ),
                                                                 MPFROM2SHORT( CMA_WINDOW, FALSE ) ), TRUE );
    check( "... 80 pels wide", left.xRight - left.xLeft, 80 );
    check( "CM_QUERYVIEWPORTRECT, right pane", (LONG)WinSendMsg( cnr, CM_QUERYVIEWPORTRECT, MPFROMP( &right ),
                                                                  MPFROM2SHORT( CMA_WINDOW, TRUE ) ), TRUE );
    check( "... right of the left one", right.xLeft > left.xRight, TRUE );
    check( "the left pane's scroll bar", WinWindowFromID( cnr, CID_HSCROLL ) != NULLHANDLE, TRUE );
    check( "the right pane's scroll bar", WinWindowFromID( cnr, CID_RIGHTHSCROLL ) != NULLHANDLE, TRUE );

    /* CM_SCROLLWINDOW across: CN_SCROLL before it returns. */
    check( "CM_SCROLLWINDOW across", (LONG)WinSendMsg( cnr, CM_SCROLLWINDOW, MPFROMSHORT( CMA_HORIZONTAL ),
                                                       MPFROMLONG( 10 ) ), TRUE );
    check( "... CN_SCROLL (CMA_HORIZONTAL)", n_hscroll, 1 );
    check( "... by 10 pels", last_inc, 10 );
    WinSendMsg( cnr, CM_SCROLLWINDOW, MPFROMSHORT( CMA_HORIZONTAL ), MPFROMLONG( -10 ) );

    /* A title edit by messages: CN_BEGINEDIT, kept unchanged. */
    memset( &ed, 0, sizeof( ed ) );
    ed.cb = sizeof( ed );
    ed.hwndCnr = cnr;
    ed.id = CID_CNRTITLEWND;
    check( "CM_OPENEDIT of the title", (LONG)WinSendMsg( cnr, CM_OPENEDIT, MPFROMP( &ed ), 0 ), TRUE );
    check( "... CN_BEGINEDIT", n_begin_title, 1 );
    check( "CM_CLOSEEDIT", (LONG)WinSendMsg( cnr, CM_CLOSEEDIT, 0, 0 ), TRUE );
    check( "... CN_ENDEDIT", n_endedit, 1 );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "CnrDetailTest", ClientProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "CnrDetailTest",
                                "Container details", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    build_container();

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( frame );
    append_str( append_long( append_str( append_long( append_str( line, "pm_cnrdetail_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
