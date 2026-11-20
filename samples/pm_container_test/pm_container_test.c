/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_container_test.c - the WC_CONTAINER control (first stage)
 *
 * A standard frame at desktop (100, 200), 400 x 300, holds in its client a
 * container at client (8, 8), 380 x 250, with extended selection. Its five
 * records are USERRECORDs (a RECORDCORE followed by a size, a date and a
 * kind), allocated with CM_ALLOCRECORD (three in one call, two singly) and
 * inserted so that the item order is
 *
 *   readme, alpha.txt, zeta.exe, beta.c, gamma.h
 *
 * The program checks the allocation (cb, the chain, zeroed fields), the
 * insertion orders (CMA_END, CMA_FIRST, after a record), CM_QUERYRECORD,
 * CM_QUERYCNRINFO / CM_SETCNRINFO, three detail-view field infos (name,
 * size, date), CM_QUERYDETAILFIELDINFO, CM_SEARCHSTRING, that
 * CM_SETRECORDEMPHASIS sends CN_EMPHASIS (a NOTIFYRECORDEMPHASIS) before
 * it returns, CM_QUERYRECORDEMPHASIS, CM_QUERYRECORDRECT, and that an
 * inserted record cannot be freed (PMERR_RECORD_CURRENTLY_INSERTED). It
 * leaves the container in the detail view with a title and column titles:
 * the rows start 39 pels below the container's top, 18 pels each, so row
 * k is centred at screen y 358 + 18 k (the container's (0, 0) is screen
 * (110, 559), screen y = 559 - y). The integration test then
 *
 *   clicks row 1 (alpha.txt, screen 140, 376);
 *   shift+clicks row 3 (beta.c, 140, 412)  -> three records selected;
 *   double clicks row 0 (readme, 140, 358) -> CN_ENTER, from the mouse;
 *   presses Down                           -> alpha.txt selected alone;
 *   presses Enter                          -> CN_ENTER, from the keyboard;
 *
 * and closes the window with Alt+F4: the selection, the most records seen
 * selected at once and the CN_ENTER records are checked, then
 * CM_REMOVERECORD (CMA_FREE) takes two records out and frees them. Each
 * check is written to stdout:
 *
 *   pm_container_test: <name>: ok
 *   pm_container_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_container_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINSTDCNR
#include <os2.h>
#include <string.h>

#define MAX_LOG 8

typedef struct _USERRECORD {
    RECORDCORE core;
    ULONG      size;
    CDATE      date;
    PSZ        kind;
} USERRECORD, *PUSERRECORD;

static HAB   hab;
static HWND  frame, client, cnr;
static PUSERRECORD rec[5];
static int   n_checks = 0;
static int   n_passed = 0;

/* CN_EMPHASIS: the last record and mask, how many, the most selected. */
static PRECORDCORE emph_rec = NULL;
static ULONG       emph_mask = 0;
static int         n_emph = 0;
static int         max_selected = 0;
/* CN_ENTER: the records and fKey. */
static PRECORDCORE enter_rec[MAX_LOG];
static ULONG       enter_key[MAX_LOG];
static int         n_enter = 0;

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
    char *p = append_str( append_str( line, "pm_container_test: " ), what );
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

/* The number of selected records (CM_QUERYRECORDEMPHASIS from the first). */
static int count_selected( void )
{
    int         n = 0;
    PRECORDCORE p = (PRECORDCORE)WinSendMsg( cnr, CM_QUERYRECORDEMPHASIS, MPFROMLONG( CMA_FIRST ),
                                             MPFROMSHORT( CRA_SELECTED ) );
    while( p != NULL && p != (PRECORDCORE)-1 && n < 10 ) {
        n++;
        p = (PRECORDCORE)WinSendMsg( cnr, CM_QUERYRECORDEMPHASIS, MPFROMP( p ), MPFROMSHORT( CRA_SELECTED ) );
    }
    return n;
}

static int selected( int i )
{
    return ( rec[i]->core.flRecordAttr & CRA_SELECTED ) != 0;
}

static void final_checks( void )
{
    check( "the most records selected at once (shift+click)", max_selected, 3 );
    check( "selected at the end: alpha.txt", selected( 0 ), TRUE );
    check( "... and nothing else", count_selected(), 1 );
    check( "alpha.txt is the cursored record", ( rec[0]->core.flRecordAttr & CRA_CURSORED ) != 0, TRUE );
    check( "CN_ENTER count", n_enter, 2 );
    check( "double click: readme", n_enter > 0 && enter_rec[0] == &rec[3]->core, TRUE );
    check( "... from the mouse", n_enter > 0 ? (LONG)enter_key[0] : -1, FALSE );
    check( "Enter: alpha.txt", n_enter > 1 && enter_rec[1] == &rec[0]->core, TRUE );
    check( "... from the keyboard", n_enter > 1 ? (LONG)enter_key[1] : -1, TRUE );
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
        if( SHORT2FROMMP( mp1 ) == CN_EMPHASIS ) {
            PNOTIFYRECORDEMPHASIS pn = (PNOTIFYRECORDEMPHASIS)mp2;
            int n;
            emph_rec = pn->pRecord;
            emph_mask = pn->fEmphasisMask;
            n_emph++;
            n = count_selected();
            if( n > max_selected ) max_selected = n;
        } else if( SHORT2FROMMP( mp1 ) == CN_ENTER && n_enter < MAX_LOG ) {
            PNOTIFYRECORDENTER pn = (PNOTIFYRECORDENTER)mp2;
            enter_rec[n_enter] = pn->pRecord;
            enter_key[n_enter] = pn->fKey;
            n_enter++;
        }
        return 0;
    case WM_CLOSE:
        final_checks();
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static void set_record( PUSERRECORD r, PSZ name, ULONG size, USHORT year, PSZ kind )
{
    r->core.pszIcon = name;
    r->core.pszText = name;
    r->core.pszName = name;
    r->core.pszTree = name;
    r->size = size;
    r->date.day = 9;
    r->date.month = 3;
    r->date.year = year;
    r->kind = kind;
}

static ULONG insert( PUSERRECORD first, ULONG n, PRECORDCORE order )
{
    RECORDINSERT ri;
    memset( &ri, 0, sizeof( ri ) );
    ri.cb = sizeof( ri );
    ri.pRecordOrder = order;
    ri.pRecordParent = NULL;
    ri.fInvalidateRecord = TRUE;
    ri.zOrder = CMA_TOP;
    ri.cRecordsInsert = n;
    return (ULONG)WinSendMsg( cnr, CM_INSERTRECORD, MPFROMP( first ), MPFROMP( &ri ) );
}

static void build_container( void )
{
    static const char *names[5] = { "alpha.txt", "beta.c", "gamma.h", "readme", "zeta.exe" };
    static const char *order[5] = { "readme", "alpha.txt", "zeta.exe", "beta.c", "gamma.h" };
    CNRINFO         ci;
    FIELDINFOINSERT fii;
    PFIELDINFO      fi, f;
    SEARCHSTRING    ss;
    QUERYRECORDRECT qrr;
    RECTL           r;
    PRECORDCORE     p;
    PRECORDCORE     arr[1];
    ULONG           err;
    int             i, ok;

    cnr = WinCreateWindow( client, WC_CONTAINER, "", WS_VISIBLE | CCS_EXTENDSEL,
                           8, 8, 380, 250, client, HWND_TOP, 1, NULL, NULL );
    check( "the container is created", cnr != NULLHANDLE, TRUE );

    /* Three records in one call, two singly. */
    rec[0] = (PUSERRECORD)WinSendMsg( cnr, CM_ALLOCRECORD,
                                      MPFROMLONG( sizeof( USERRECORD ) - sizeof( RECORDCORE ) ), MPFROMSHORT( 3 ) );
    check( "CM_ALLOCRECORD", rec[0] != NULL, TRUE );
    rec[1] = (PUSERRECORD)rec[0]->core.preccNextRecord;
    rec[2] = (PUSERRECORD)rec[1]->core.preccNextRecord;
    check( "the records are chained", rec[2] != NULL && rec[2]->core.preccNextRecord == NULL, TRUE );
    check( "cb is the RECORDCORE size", (LONG)rec[0]->core.cb, sizeof( RECORDCORE ) );
    check( "the records are zeroed", rec[1]->size == 0 && rec[1]->core.pszText == NULL
                                     && rec[1]->core.flRecordAttr == 0, TRUE );
    for( i = 3; i < 5; i++ ) {
        rec[i] = (PUSERRECORD)WinSendMsg( cnr, CM_ALLOCRECORD,
                                          MPFROMLONG( sizeof( USERRECORD ) - sizeof( RECORDCORE ) ), MPFROMSHORT( 1 ) );
    }
    check( "single records", rec[3] != NULL && rec[4] != NULL && rec[3]->core.preccNextRecord == NULL, TRUE );
    for( i = 0; i < 5; i++ ) {
        set_record( rec[i], (PSZ)names[i], 1000 * ( i + 1 ), (USHORT)( 2020 + i ), "file" );
    }

    check( "insert three at the end", insert( rec[0], 3, (PRECORDCORE)CMA_END ), 3 );
    check( "insert one first", insert( rec[3], 1, (PRECORDCORE)CMA_FIRST ), 4 );
    check( "insert one after alpha.txt", insert( rec[4], 1, &rec[0]->core ), 5 );
    check( "inserting again fails", insert( rec[4], 1, (PRECORDCORE)CMA_END ), 0 );

    p = (PRECORDCORE)WinSendMsg( cnr, CM_QUERYRECORD, 0, MPFROM2SHORT( CMA_FIRST, CMA_ITEMORDER ) );
    ok = TRUE;
    for( i = 0; i < 5; i++ ) {
        if( p == NULL || strcmp( p->pszText, order[i] ) != 0 ) ok = FALSE;
        if( p != NULL ) p = (PRECORDCORE)WinSendMsg( cnr, CM_QUERYRECORD, MPFROMP( p ),
                                                     MPFROM2SHORT( CMA_NEXT, CMA_ITEMORDER ) );
    }
    check( "item order (CM_QUERYRECORD)", ok && p == NULL, TRUE );
    check( "the last record", (LONG)WinSendMsg( cnr, CM_QUERYRECORD, 0, MPFROM2SHORT( CMA_LAST, CMA_ITEMORDER ) ),
           (LONG)rec[2] );

    /* An inserted record cannot be freed. */
    arr[0] = &rec[1]->core;
    WinGetLastError( hab );
    check( "CM_FREERECORD of an inserted record", (LONG)WinSendMsg( cnr, CM_FREERECORD, MPFROMP( arr ), MPFROMSHORT( 1 ) ),
           FALSE );
    err = WinGetLastError( hab );
    check( "... PMERR_RECORD_CURRENTLY_INSERTED", ERRORIDERROR( err ), PMERR_RECORD_CURRENTLY_INSERTED );

    /* The detail view: a title, column titles, three columns. */
    fi = (PFIELDINFO)WinSendMsg( cnr, CM_ALLOCDETAILFIELDINFO, MPFROMSHORT( 3 ), 0 );
    check( "CM_ALLOCDETAILFIELDINFO", fi != NULL && fi->cb == sizeof( FIELDINFO ), TRUE );
    f = fi;
    f->flData = CFA_STRING | CFA_LEFT | CFA_SEPARATOR | CFA_HORZSEPARATOR;
    f->flTitle = CFA_LEFT;
    f->pTitleData = "Name";
    f->offStruct = FIELDOFFSET( USERRECORD, core.pszName );
    f = f->pNextFieldInfo;
    f->flData = CFA_ULONG | CFA_RIGHT | CFA_SEPARATOR | CFA_HORZSEPARATOR;
    f->flTitle = CFA_RIGHT;
    f->pTitleData = "Size";
    f->offStruct = FIELDOFFSET( USERRECORD, size );
    f = f->pNextFieldInfo;
    f->flData = CFA_DATE | CFA_LEFT | CFA_HORZSEPARATOR;
    f->flTitle = CFA_LEFT;
    f->pTitleData = "Date";
    f->offStruct = FIELDOFFSET( USERRECORD, date );
    memset( &fii, 0, sizeof( fii ) );
    fii.cb = sizeof( fii );
    fii.pFieldInfoOrder = (PFIELDINFO)CMA_END;
    fii.fInvalidateFieldInfo = TRUE;
    fii.cFieldInfoInsert = 3;
    check( "CM_INSERTDETAILFIELDINFO", (LONG)WinSendMsg( cnr, CM_INSERTDETAILFIELDINFO, MPFROMP( fi ), MPFROMP( &fii ) ), 3 );
    check( "first field info", (LONG)WinSendMsg( cnr, CM_QUERYDETAILFIELDINFO, 0, MPFROMSHORT( CMA_FIRST ) ), (LONG)fi );
    check( "next field info", (LONG)WinSendMsg( cnr, CM_QUERYDETAILFIELDINFO, MPFROMP( fi ), MPFROMSHORT( CMA_NEXT ) ),
           (LONG)fi->pNextFieldInfo );

    memset( &ci, 0, sizeof( ci ) );
    ci.cb = sizeof( ci );
    ci.flWindowAttr = CV_DETAIL | CA_DETAILSVIEWTITLES | CA_CONTAINERTITLE | CA_TITLESEPARATOR | CA_TITLELEFT;
    ci.pszCnrTitle = "Files";
    check( "CM_SETCNRINFO", (LONG)WinSendMsg( cnr, CM_SETCNRINFO, MPFROMP( &ci ),
                                              MPFROMLONG( CMA_FLWINDOWATTR | CMA_CNRTITLE ) ), TRUE );
    memset( &ci, 0, sizeof( ci ) );
    check( "CM_QUERYCNRINFO", (LONG)WinSendMsg( cnr, CM_QUERYCNRINFO, MPFROMP( &ci ), MPFROMLONG( sizeof( ci ) ) ),
           sizeof( ci ) );
    check( "... records", (LONG)ci.cRecords, 5 );
    check( "... fields", (LONG)ci.cFields, 3 );
    check( "... view", (LONG)( ci.flWindowAttr & CV_DETAIL ), CV_DETAIL );
    check( "... title", ci.pszCnrTitle != NULL && strcmp( ci.pszCnrTitle, "Files" ) == 0, TRUE );

    /* CM_SEARCHSTRING by prefix, without case. */
    memset( &ss, 0, sizeof( ss ) );
    ss.cb = sizeof( ss );
    ss.pszSearch = "GAM";
    ss.fsPrefix = TRUE;
    ss.fsCaseSensitive = FALSE;
    ss.usView = CV_TEXT;
    check( "CM_SEARCHSTRING", (LONG)WinSendMsg( cnr, CM_SEARCHSTRING, MPFROMP( &ss ), MPFROMLONG( CMA_FIRST ) ),
           (LONG)rec[2] );

    /* Emphasis: CN_EMPHASIS before CM_SETRECORDEMPHASIS returns. */
    check( "CM_SETRECORDEMPHASIS", (LONG)WinSendMsg( cnr, CM_SETRECORDEMPHASIS, MPFROMP( rec[4] ),
                                                     MPFROM2SHORT( TRUE, CRA_SELECTED ) ), TRUE );
    check( "CN_EMPHASIS already received", n_emph, 1 );
    check( "... for zeta.exe", emph_rec == &rec[4]->core, TRUE );
    check( "... selection changed", (LONG)emph_mask, CRA_SELECTED );
    check( "flRecordAttr shows it", selected( 4 ), TRUE );
    check( "CM_QUERYRECORDEMPHASIS", (LONG)WinSendMsg( cnr, CM_QUERYRECORDEMPHASIS, MPFROMLONG( CMA_FIRST ),
                                                       MPFROMSHORT( CRA_SELECTED ) ), (LONG)rec[4] );
    WinSendMsg( cnr, CM_SETRECORDEMPHASIS, MPFROMP( rec[4] ), MPFROM2SHORT( FALSE, CRA_SELECTED ) );
    check( "deselected", count_selected(), 0 );
    max_selected = 0;

    /* The rectangle of the second row (alpha.txt). */
    memset( &qrr, 0, sizeof( qrr ) );
    qrr.cb = sizeof( qrr );
    qrr.pRecord = &rec[0]->core;
    qrr.fsExtent = CMA_TEXT;
    check( "CM_QUERYRECORDRECT", (LONG)WinSendMsg( cnr, CM_QUERYRECORDRECT, MPFROMP( &r ), MPFROMP( &qrr ) ), TRUE );
    check( "... second row from the top", r.yTop, 193 );
    check( "... row height", r.yTop - r.yBottom, 18 );
}

int main( void )
{
    HMQ         hmq;
    QMSG        qmsg;
    ULONG       flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    PRECORDCORE arr[2];
    char        line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "ContainerTest", ClientProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "ContainerTest",
                                "Container", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    build_container();

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    /* Remove two records and free them. */
    arr[0] = &rec[1]->core;
    arr[1] = &rec[2]->core;
    check( "CM_REMOVERECORD (CMA_FREE)", (LONG)WinSendMsg( cnr, CM_REMOVERECORD, MPFROMP( arr ),
                                                           MPFROM2SHORT( 2, CMA_FREE | CMA_INVALIDATE ) ), 3 );
    check( "the first record after it", (LONG)WinSendMsg( cnr, CM_QUERYRECORD, 0,
                                                          MPFROM2SHORT( CMA_FIRST, CMA_ITEMORDER ) ), (LONG)rec[3] );
    check( "remove all", (LONG)WinSendMsg( cnr, CM_REMOVERECORD, 0, MPFROM2SHORT( 0, CMA_FREE ) ), 0 );

    WinDestroyWindow( frame );
    append_str( append_long( append_str( append_long( append_str( line, "pm_container_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
