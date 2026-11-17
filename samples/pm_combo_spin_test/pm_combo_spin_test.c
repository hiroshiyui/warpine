/*
 * pm_combo_spin_test.c - combination boxes, spin buttons and owner drawing
 * under Warpine
 *
 * The client (default frame at desktop (32, 32), title bar only) holds:
 *
 *   combo 10   CBS_DROPDOWN      at ( 20, 250) 160 x 120: Red Green Blue Yellow
 *   combo 11   CBS_DROPDOWNLIST  at (220, 250) 160 x 120
 *   combo 12   CBS_SIMPLE        at (420, 250) 160 x 120
 *   spin  20   SPBS_MASTER | SPBS_NUMERICONLY | SPBS_JUSTRIGHT
 *                                at ( 20, 200) 100 x  24: 0..10, current 5
 *   spin  21   SPBS_MASTER | SPBS_READONLY
 *                                at (140, 200) 120 x  24: Mon .. Sun
 *   list  30   LS_OWNERDRAW      at ( 20,  40) 200 x 140: four colours,
 *              24-pel items (WM_MEASUREITEM); the owner paints a swatch and
 *              the name, the selection itself, and leaves item 3 to the
 *              list box (returns FALSE)
 *   button 40  BS_USERBUTTON | BS_DEFAULT at (260, 100) 120 x 40, painted
 *              by the owner on BN_PAINT
 *
 * main() checks the messages first; the integration test then clicks the
 * drop-down arrow of combo 10 and its second item, the up arrow of spin 20,
 * the down arrow of spin 21 and the second item of the owner-drawn list,
 * drops combo 10 again and closes the window with Alt+F4. The
 * notifications are logged and checked at the end. Each check is written
 * to stdout:
 *
 *   pm_combo_spin_test: <name>: ok
 *   pm_combo_spin_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_combo_spin_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINSTDSPIN
#define INCL_DOSPROCESS
#include <os2.h>
#include <string.h>

#define ID_DROPDOWN  10
#define ID_DROPLIST  11
#define ID_SIMPLE    12
#define ID_NUMSPIN   20
#define ID_DAYSPIN   21
#define ID_OWNERLIST 30
#define ID_USERBTN   40
#define ITEM_H       24

static HAB  hab;
static HWND frame, client, dropdown, droplist, simple, numspin, dayspin, ownerlist, userbtn;
static int  n_checks = 0;
static int  n_passed = 0;

/* Counts of what the owner was sent. */
static int n_showlist, n_lbselect, n_efchange, n_upspin, n_downspin, n_endspin, n_spinchange;
static int n_measure, n_drawitem, n_bnpaint, n_lnselect;
static LONG measured_item = -1;
static ULONG last_paint_state = 0xFFFF;
static ULONG drawn_selected = 0xFFFF;    /* fsState of item 1 when last drawn */
static ULONG drawn_handle = 0;           /* hItem of item 2 when last drawn */
static LONG  drawn_height = 0;           /* rclItem height */
static LONG  drawn_left = -1;            /* rclItem.xLeft */

static const char *colours[] = { "Red", "Green", "Blue", "Yellow" };
static const LONG  clr[] = { CLR_RED, CLR_GREEN, CLR_BLUE, CLR_YELLOW };
static PSZ days[] = { "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun" };

/* ---------- output (no libc printf) ---------- */

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
    char  line[200], *p;
    p = append_str( line, "pm_combo_spin_test: " );
    p = append_str( p, what );
    if( got == want ) {
        append_str( p, ": ok\r\n" );
        n_passed++;
    } else {
        p = append_str( p, ": FAIL got " );
        p = append_long( p, got );
        p = append_str( p, " want " );
        p = append_long( p, want );
        append_str( p, "\r\n" );
    }
    write_stdout( line );
    n_checks++;
}

static void check_text( const char *what, HWND hwnd, const char *want )
{
    char buf[64];
    WinQueryWindowText( hwnd, sizeof( buf ), buf );
    check( what, strcmp( buf, want ) == 0, TRUE );
}

/* ---------- the client ---------- */

static void draw_item( POWNERITEM poi )
{
    RECTL  rc = poi->rclItem, swatch;
    POINTL pt;
    LONG   i = poi->idItem;
    char   text[32];

    if( i == 1 ) drawn_selected = poi->fsState;
    if( i == 2 ) drawn_handle = poi->hItem;
    drawn_height = rc.yTop - rc.yBottom;
    drawn_left = rc.xLeft;
    WinFillRect( poi->hps, &rc, poi->fsState ? CLR_DARKGRAY : CLR_WHITE );
    swatch.xLeft = rc.xLeft + 4;
    swatch.yBottom = rc.yBottom + 4;
    swatch.xRight = rc.xLeft + 30;
    swatch.yTop = rc.yTop - 4;
    WinFillRect( poi->hps, &swatch, clr[i & 3] );
    WinSendMsg( poi->hwnd, LM_QUERYITEMTEXT, MPFROM2SHORT( i, sizeof( text ) ), MPFROMP( text ) );
    GpiSetColor( poi->hps, poi->fsState ? CLR_WHITE : CLR_BLACK );
    pt.x = rc.xLeft + 40;
    pt.y = rc.yBottom + 7;
    GpiCharStringAt( poi->hps, &pt, strlen( text ), text );
    /* The owner shows the selection itself. */
    poi->fsState = poi->fsStateOld = FALSE;
}

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_MEASUREITEM:
        if( SHORT1FROMMP( mp1 ) == ID_OWNERLIST ) {
            n_measure++;
            if( measured_item < 0 ) measured_item = (LONG)mp2;
            return MRFROM2SHORT( ITEM_H, 0 );
        }
        break;

    case WM_DRAWITEM:
        if( SHORT1FROMMP( mp1 ) == ID_OWNERLIST ) {
            POWNERITEM poi = (POWNERITEM)mp2;
            n_drawitem++;
            if( poi->idItem == 3 ) return (MRESULT)FALSE;   /* the list box draws it */
            draw_item( poi );
            return (MRESULT)TRUE;
        }
        break;

    case WM_CONTROL:
        switch( SHORT1FROMMP( mp1 ) ) {
        case ID_DROPDOWN:
            switch( SHORT2FROMMP( mp1 ) ) {
            case CBN_SHOWLIST: n_showlist++; break;
            case CBN_LBSELECT: n_lbselect++; break;
            case CBN_EFCHANGE: n_efchange++; break;
            }
            return 0;
        case ID_NUMSPIN:
        case ID_DAYSPIN:
            switch( SHORT2FROMMP( mp1 ) ) {
            case SPBN_UPARROW:   n_upspin++; break;
            case SPBN_DOWNARROW: n_downspin++; break;
            case SPBN_ENDSPIN:   n_endspin++; break;
            case SPBN_CHANGE:    n_spinchange++; break;
            }
            return 0;
        case ID_OWNERLIST:
            if( SHORT2FROMMP( mp1 ) == LN_SELECT ) n_lnselect++;
            return 0;
        case ID_USERBTN:
            if( SHORT2FROMMP( mp1 ) == BN_PAINT ) {
                PUSERBUTTON pub = (PUSERBUTTON)mp2;
                RECTL rc;
                POINTL pt;
                n_bnpaint++;
                last_paint_state = pub->fsState;
                WinQueryWindowRect( pub->hwnd, &rc );
                WinFillRect( pub->hps, &rc, ( pub->fsState & BDS_HILITED ) ? CLR_DARKBLUE : CLR_CYAN );
                GpiSetColor( pub->hps, CLR_BLACK );
                pt.x = 12;
                pt.y = 14;
                GpiCharStringAt( pub->hps, &pt, 11, "User button" );
            }
            return 0;
        }
        return 0;

    case WM_PAINT: {
        RECTL rc;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinFillRect( hps, &rc, CLR_WHITE );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static HWND control( PSZ cls, ULONG style, LONG x, LONG y, LONG cx, LONG cy, ULONG id )
{
    return WinCreateWindow( client, cls, NULL, WS_VISIBLE | style, x, y, cx, cy,
                            client, HWND_TOP, id, NULL, NULL );
}

static LONG spin_value( HWND spin )
{
    LONG v = -1;
    WinSendMsg( spin, SPBM_QUERYVALUE, MPFROMP( &v ), MPFROM2SHORT( 0, SPBQ_UPDATEIFVALID ) );
    return v;
}

/* ---------- checks before the session ---------- */

static void check_combos( void )
{
    int  i;
    char buf[32];

    for( i = 0; i < 4; i++ ) {
        WinSendMsg( dropdown, LM_INSERTITEM, MPFROMSHORT( LIT_END ), MPFROMP( colours[i] ) );
        WinSendMsg( droplist, LM_INSERTITEM, MPFROMSHORT( LIT_SORTASCENDING ), MPFROMP( colours[i] ) );
        WinSendMsg( simple, LM_INSERTITEM, MPFROMSHORT( LIT_END ), MPFROMP( colours[i] ) );
    }
    check( "combo parts: entry field", WinWindowFromID( dropdown, CBID_EDIT ) != NULLHANDLE, TRUE );
    check( "combo parts: list box", WinWindowFromID( dropdown, CBID_LIST ) != NULLHANDLE, TRUE );
    check( "LM_QUERYITEMCOUNT through the combo",
           (LONG)WinSendMsg( dropdown, LM_QUERYITEMCOUNT, 0, 0 ), 4 );
    check( "LM_QUERYITEMTEXT through the combo",
           (LONG)WinSendMsg( droplist, LM_QUERYITEMTEXT, MPFROM2SHORT( 0, sizeof( buf ) ), MPFROMP( buf ) ), 4 );
    check( "  sorted: Blue first", strcmp( buf, "Blue" ) == 0, TRUE );
    check( "LM_SEARCHSTRING prefix",
           (LONG)WinSendMsg( dropdown, LM_SEARCHSTRING, MPFROM2SHORT( LSS_PREFIX, LIT_FIRST ), MPFROMP( "ye" ) ), 3 );
    check( "CBM_ISLISTSHOWING drop-down", (LONG)WinSendMsg( dropdown, CBM_ISLISTSHOWING, 0, 0 ), FALSE );
    check( "CBM_ISLISTSHOWING simple", (LONG)WinSendMsg( simple, CBM_ISLISTSHOWING, 0, 0 ), TRUE );
    check( "CBM_SHOWLIST on CBS_SIMPLE", (LONG)WinSendMsg( simple, CBM_SHOWLIST, MPFROMSHORT( TRUE ), 0 ), FALSE );
    check( "CBM_SHOWLIST shows", (LONG)WinSendMsg( droplist, CBM_SHOWLIST, MPFROMSHORT( TRUE ), 0 ), TRUE );
    check( "CBM_SHOWLIST again", (LONG)WinSendMsg( droplist, CBM_SHOWLIST, MPFROMSHORT( TRUE ), 0 ), FALSE );
    check( "CBM_SHOWLIST hides", (LONG)WinSendMsg( droplist, CBM_SHOWLIST, MPFROMSHORT( FALSE ), 0 ), TRUE );
    check( "CBM_HILITE", (LONG)WinSendMsg( simple, CBM_HILITE, MPFROMSHORT( TRUE ), 0 ), TRUE );
    check( "CBM_HILITE again", (LONG)WinSendMsg( simple, CBM_HILITE, MPFROMSHORT( TRUE ), 0 ), FALSE );
    WinSendMsg( simple, CBM_HILITE, MPFROMSHORT( FALSE ), 0 );
    check( "drop-down list field is read-only",
           (LONG)WinSendMsg( droplist, EM_QUERYREADONLY, 0, 0 ), TRUE );
    WinSendMsg( dropdown, LM_SELECTITEM, MPFROMSHORT( 0 ), MPFROMSHORT( TRUE ) );
    check_text( "LM_SELECTITEM fills the field", dropdown, "Red" );
    WinSendMsg( simple, LM_SELECTITEM, MPFROMSHORT( 2 ), MPFROMSHORT( TRUE ) );
    check_text( "CBS_SIMPLE selection", simple, "Blue" );
    WinSetWindowText( droplist, "Green" );
    check_text( "WinSetWindowText on the combo", droplist, "Green" );
}

static void check_spins( void )
{
    LONG hi = 0, lo = 0;
    char buf[16];

    check( "SPBM_SETLIMITS", (LONG)WinSendMsg( numspin, SPBM_SETLIMITS, MPFROMLONG( 10 ), MPFROMLONG( 0 ) ), TRUE );
    check( "SPBM_SETLIMITS upper < lower",
           (LONG)WinSendMsg( numspin, SPBM_SETLIMITS, MPFROMLONG( 0 ), MPFROMLONG( 10 ) ), FALSE );
    check( "SPBM_QUERYLIMITS", (LONG)WinSendMsg( numspin, SPBM_QUERYLIMITS, MPFROMP( &hi ), MPFROMP( &lo ) ), TRUE );
    check( "  upper", hi, 10 );
    check( "  lower", lo, 0 );
    check( "SPBM_SETCURRENTVALUE 8", (LONG)WinSendMsg( numspin, SPBM_SETCURRENTVALUE, MPFROMLONG( 8 ), 0 ), TRUE );
    check( "SPBM_SETCURRENTVALUE out of range",
           (LONG)WinSendMsg( numspin, SPBM_SETCURRENTVALUE, MPFROMLONG( 11 ), 0 ), FALSE );
    check( "SPBM_SPINUP 4 wraps round", (LONG)WinSendMsg( numspin, SPBM_SPINUP, MPFROMLONG( 4 ), 0 ), TRUE );
    check( "  value", spin_value( numspin ), 1 );
    WinSendMsg( numspin, SPBM_SPINDOWN, MPFROMLONG( 2 ), 0 );
    check( "SPBM_SPINDOWN 2 wraps round", spin_value( numspin ), 10 );
    WinSetWindowText( numspin, "7" );
    check( "SPBM_QUERYVALUE reads the field", spin_value( numspin ), 7 );
    WinSetWindowText( numspin, "x" );
    check( "SPBM_QUERYVALUE of rubbish",
           (LONG)WinSendMsg( numspin, SPBM_QUERYVALUE, MPFROMP( &lo ), MPFROM2SHORT( 0, SPBQ_ALWAYSUPDATE ) ), FALSE );
    check_text( "SPBQ_ALWAYSUPDATE puts the value back", numspin, "7" );
    WinSendMsg( numspin, SPBM_SETCURRENTVALUE, MPFROMLONG( 5 ), 0 );

    check( "SPBM_SETARRAY", (LONG)WinSendMsg( dayspin, SPBM_SETARRAY, MPFROMP( days ), MPFROMLONG( 7 ) ), TRUE );
    check_text( "array: first item", dayspin, "Mon" );
    WinSendMsg( dayspin, SPBM_SPINUP, MPFROMLONG( 2 ), 0 );
    check( "array: index after SPBM_SPINUP 2", spin_value( dayspin ), 2 );
    WinSendMsg( dayspin, SPBM_QUERYVALUE, MPFROMP( buf ), MPFROM2SHORT( sizeof( buf ), SPBQ_DONOTUPDATE ) );
    check( "array: text from SPBM_QUERYVALUE", strcmp( buf, "Wed" ) == 0, TRUE );
    WinSendMsg( dayspin, SPBM_SETCURRENTVALUE, MPFROMLONG( 0 ), 0 );
}

static void check_owner_list( void )
{
    int i;
    for( i = 0; i < 4; i++ )
        WinSendMsg( ownerlist, LM_INSERTITEM, MPFROMSHORT( LIT_END ), MPFROMP( colours[i] ) );
    WinSendMsg( ownerlist, LM_SETITEMHANDLE, MPFROMSHORT( 2 ), MPFROMLONG( 0xBEEF ) );
    check( "LM_QUERYITEMHANDLE", (LONG)WinSendMsg( ownerlist, LM_QUERYITEMHANDLE, MPFROMSHORT( 2 ), 0 ), 0xBEEF );
    check( "WM_MEASUREITEM sent", n_measure > 0, TRUE );
    check( "  for item 0 first", measured_item, 0 );
    check( "WM_DRAWITEM sent", n_drawitem >= 4, TRUE );
    check( "  rclItem one measured row", drawn_height, ITEM_H );
    check( "  rclItem relative to the list box", drawn_left, 1 );
}

/* ---------- checks after the session ---------- */

static void check_session( void )
{
    check( "CBN_SHOWLIST for both drops", n_showlist, 2 );
    check( "CBN_LBSELECT after the pick", n_lbselect >= 1, TRUE );
    check( "CBN_EFCHANGE after the pick", n_efchange >= 1, TRUE );
    check_text( "picked item in the field", dropdown, "Green" );
    check( "list showing at the end", (LONG)WinSendMsg( dropdown, CBM_ISLISTSHOWING, 0, 0 ), TRUE );
    check( "up arrow spun 5 -> 6", spin_value( numspin ), 6 );
    check_text( "down arrow wrapped Mon -> Sun", dayspin, "Sun" );
    check( "SPBN_UPARROW", n_upspin, 1 );
    check( "SPBN_DOWNARROW", n_downspin, 1 );
    check( "SPBN_ENDSPIN", n_endspin, 2 );
    check( "SPBN_CHANGE", n_spinchange >= 2, TRUE );
    check( "owner list: LN_SELECT", n_lnselect, 1 );
    check( "owner list: item 1 selected",
           (LONG)WinSendMsg( ownerlist, LM_QUERYSELECTION, MPFROMSHORT( LIT_FIRST ), 0 ), 1 );
    check( "owner list: item 1 drawn selected", drawn_selected, TRUE );
    check( "owner list: hItem of item 2", drawn_handle, 0xBEEF );
    check( "BN_PAINT sent", n_bnpaint > 0, TRUE );
    check( "  fsState BDS_DEFAULT", last_paint_state, BDS_DEFAULT );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_BORDER;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "ComboSpinTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "ComboSpinTest",
                                "PM Combo / Spin / Owner Draw Test", WS_VISIBLE, NULLHANDLE, 0, &client );

    dropdown  = control( WC_COMBOBOX, CBS_DROPDOWN | WS_TABSTOP, 20, 250, 160, 120, ID_DROPDOWN );
    droplist  = control( WC_COMBOBOX, CBS_DROPDOWNLIST, 220, 250, 160, 120, ID_DROPLIST );
    simple    = control( WC_COMBOBOX, CBS_SIMPLE, 420, 250, 160, 120, ID_SIMPLE );
    numspin   = control( WC_SPINBUTTON, SPBS_MASTER | SPBS_NUMERICONLY | SPBS_JUSTRIGHT, 20, 200, 100, 24, ID_NUMSPIN );
    dayspin   = control( WC_SPINBUTTON, SPBS_MASTER | SPBS_READONLY, 140, 200, 120, 24, ID_DAYSPIN );
    ownerlist = control( WC_LISTBOX, LS_OWNERDRAW, 20, 40, 200, 140, ID_OWNERLIST );
    userbtn   = control( WC_BUTTON, BS_USERBUTTON | BS_DEFAULT, 260, 100, 120, 40, ID_USERBTN );
    check( "WinCreateWindow", dropdown && droplist && simple && numspin && dayspin && ownerlist && userbtn, TRUE );

    check_combos();
    check_spins();
    check_owner_list();
    n_showlist = n_lbselect = n_efchange = 0;
    n_upspin = n_downspin = n_endspin = n_spinchange = n_lnselect = 0;
    /* Notifications of the checks above are still queued: drain them first. */
    while( WinPeekMsg( hab, &qmsg, NULLHANDLE, 0, 0, PM_REMOVE ) ) {
        if( qmsg.msg == WM_QUIT ) break;
        WinDispatchMsg( hab, &qmsg );
    }
    n_showlist = n_lbselect = n_efchange = 0;
    n_upspin = n_downspin = n_endspin = n_spinchange = n_lnselect = 0;

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    check_session();

    p = append_str( line, "pm_combo_spin_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
