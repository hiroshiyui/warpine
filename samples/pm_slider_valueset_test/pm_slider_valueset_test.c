/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_slider_valueset_test.c - slider and value set controls under Warpine
 *
 * The client (default frame at desktop (32, 32), title bar only) holds:
 *
 *   slider 10  SLS_BUTTONSLEFT | SLS_SNAPTOINCREMENT | SLS_RIBBONSTRIP
 *              at ( 20, 330) 300 x 60: scale 1 = 6 increments 40 pels
 *              apart (range 200), scale 2 = 3 increments; ticks of 6 pels
 *              labelled 0..5, a detent at 180
 *   slider 11  SLS_VERTICAL | SLS_BUTTONSTOP | SLS_PRIMARYSCALE2
 *              at (360, 200) 60 x 200: scale 2 = 11 increments, spacing 0
 *   slider 12  SLS_READONLY | SLS_RIBBONSTRIP at (20, 280) 300 x 40: a
 *              progress bar at increment 6 of 10
 *   slider 13  SLS_OWNERDRAW at (20, 220) 300 x 50: the owner draws the
 *              arm (red) and leaves the other parts to the slider
 *   valueset 20 VS_RGB | VS_BORDER | VS_ITEMBORDER, 2 x 4, at (20, 40)
 *              240 x 100: eight colours
 *   valueset 21 VS_TEXT | VS_RIGHTTOLEFT, 1 x 3, at (280, 40) 180 x 40:
 *              One Two Three, item 2 disabled
 *   valueset 22 VS_COLORINDEX, 1 x 4, at (280, 100) 180 x 40: item 2 drawn
 *              by the owner
 *
 * main() checks the messages first; the integration test then clicks the
 * shaft of slider 10 right of its arm, drags the arm, clicks its right
 * button, presses Left and Shift+Right, clicks item (2, 3) of value set 20
 * and presses Right, double-clicks its item (1, 1), clicks the leftmost item
 * of value set 21 (column 3) and closes the window with Alt+F4. The
 * notifications are logged and checked at the end. Each check is written to
 * stdout:
 *
 *   pm_slider_valueset_test: <name>: ok
 *   pm_slider_valueset_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_slider_valueset_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINSTDSLIDER
#define INCL_WINSTDVALSET
#define INCL_DOSPROCESS
#include <os2.h>
#include <string.h>

#define ID_SLIDER    10
#define ID_VSLIDER   11
#define ID_PROGRESS  12
#define ID_OWNSLIDER 13
#define ID_COLORS    20
#define ID_TEXTS     21
#define ID_INDICES   22

static HAB  hab;
static HWND frame, client, slider, vslider, progress, ownslider, colors, texts, indices;
static int  n_checks = 0;
static int  n_passed = 0;

/* What the owner was told during the session. */
static int   n_change, n_track, n_setfocus, n_killfocus;
static LONG  last_change = -1;
static int   n_vselect, n_venter, n_vsetfocus;
static ULONG last_vselect = 0, last_venter = 0, text_select = 0;
/* Owner drawing. */
static int   n_draw_arm, n_draw_other, n_draw_item;
static LONG  arm_width = -1;
static ULONG drawn_hitem = 0;
static LONG  drawn_item_width = -1;

static const ULONG rgb[8] = { 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00,
                              0xFF00FF, 0x00FFFF, 0x808080, 0x000000 };
static const char *labels[6] = { "0", "1", "2", "3", "4", "5" };

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
    p = append_str( line, "pm_slider_valueset_test: " );
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

/* ---------- the client ---------- */

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_DRAWITEM: {
        POWNERITEM poi = (POWNERITEM)mp2;
        if( SHORT1FROMMP( mp1 ) == ID_OWNSLIDER ) {
            if( poi->idItem == SDA_SLIDERARM ) {
                n_draw_arm++;
                arm_width = poi->rclItem.xRight - poi->rclItem.xLeft;
                WinFillRect( poi->hps, &poi->rclItem, CLR_RED );
                return (MRESULT)TRUE;
            }
            n_draw_other++;
            return (MRESULT)FALSE;          /* the slider draws it */
        }
        if( SHORT1FROMMP( mp1 ) == ID_INDICES && poi->idItem == VDA_ITEM ) {
            RECTL inner = poi->rclItem;
            n_draw_item++;
            drawn_hitem = poi->hItem;
            drawn_item_width = poi->rclItem.xRight - poi->rclItem.xLeft;
            WinFillRect( poi->hps, &poi->rclItem, CLR_DARKCYAN );
            inner.xLeft += 6; inner.xRight -= 6; inner.yBottom += 6; inner.yTop -= 6;
            WinFillRect( poi->hps, &inner, CLR_YELLOW );
            return (MRESULT)TRUE;
        }
        break;
    }

    case WM_CONTROL:
        switch( SHORT1FROMMP( mp1 ) ) {
        case ID_SLIDER:
            switch( SHORT2FROMMP( mp1 ) ) {
            case SLN_CHANGE:      n_change++; last_change = (LONG)mp2; break;
            case SLN_SLIDERTRACK: n_track++; break;
            case SLN_SETFOCUS:    n_setfocus++; break;
            case SLN_KILLFOCUS:   n_killfocus++; break;
            }
            return 0;
        case ID_COLORS:
            switch( SHORT2FROMMP( mp1 ) ) {
            case VN_SELECT:   n_vselect++; last_vselect = (ULONG)mp2; break;
            case VN_ENTER:    n_venter++; last_venter = (ULONG)mp2; break;
            case VN_SETFOCUS: n_vsetfocus++; break;
            }
            return 0;
        case ID_TEXTS:
            if( SHORT2FROMMP( mp1 ) == VN_SELECT ) text_select = (ULONG)mp2;
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

static HWND control( PSZ cls, ULONG style, LONG x, LONG y, LONG cx, LONG cy, ULONG id, PVOID ctl )
{
    return WinCreateWindow( client, cls, NULL, WS_VISIBLE | style, x, y, cx, cy,
                            client, HWND_TOP, id, ctl, NULL );
}

static HWND make_slider( ULONG style, LONG x, LONG y, LONG cx, LONG cy, ULONG id,
                         USHORT inc1, USHORT sp1, USHORT inc2, USHORT sp2 )
{
    SLDCDATA sld;
    sld.cbSize = sizeof( SLDCDATA );
    sld.usScale1Increments = inc1;
    sld.usScale1Spacing = sp1;
    sld.usScale2Increments = inc2;
    sld.usScale2Spacing = sp2;
    return control( WC_SLIDER, style, x, y, cx, cy, id, &sld );
}

static HWND make_valueset( ULONG style, LONG x, LONG y, LONG cx, LONG cy, ULONG id, USHORT rows, USHORT cols )
{
    VSCDATA vsc;
    vsc.cbSize = sizeof( VSCDATA );
    vsc.usRowCount = rows;
    vsc.usColumnCount = cols;
    return control( WC_VALUESET, style, x, y, cx, cy, id, &vsc );
}

static LONG arm_pos( HWND s )
{
    return SHORT1FROMMR( WinSendMsg( s, SLM_QUERYSLIDERINFO,
                                     MPFROM2SHORT( SMA_SLIDERARMPOSITION, SMA_RANGEVALUE ), 0 ) );
}

/* ---------- checks before the session ---------- */

static void check_sliders( void )
{
    MRESULT mr;
    char    buf[16];
    ULONG   detent, extra;
    int     i;

    mr = WinSendMsg( slider, SLM_QUERYSLIDERINFO, MPFROM2SHORT( SMA_SHAFTDIMENSIONS, 0 ), 0 );
    check( "shaft length from spacing (range 200 + arm)", SHORT1FROMMR( mr ) > 200, TRUE );
    mr = WinSendMsg( slider, SLM_QUERYSLIDERINFO, MPFROM2SHORT( SMA_SLIDERARMPOSITION, SMA_RANGEVALUE ), 0 );
    check( "arm at home", SHORT1FROMMR( mr ), 0 );
    check( "  range in pixels", SHORT2FROMMR( mr ), 200 );
    check( "SLM_SETTICKSIZE all",
           (LONG)WinSendMsg( slider, SLM_SETTICKSIZE, MPFROM2SHORT( SMA_SETALLTICKS, 6 ), 0 ), TRUE );
    check( "SLM_QUERYTICKSIZE", (LONG)WinSendMsg( slider, SLM_QUERYTICKSIZE, MPFROMSHORT( 3 ), 0 ), 6 );
    check( "SLM_SETTICKSIZE past the scale",
           (LONG)WinSendMsg( slider, SLM_SETTICKSIZE, MPFROM2SHORT( 6, 6 ), 0 ), FALSE );
    for( i = 0; i < 6; i++ )
        WinSendMsg( slider, SLM_SETSCALETEXT, MPFROMSHORT( i ), MPFROMP( labels[i] ) );
    check( "SLM_QUERYSCALETEXT",
           (LONG)WinSendMsg( slider, SLM_QUERYSCALETEXT, MPFROM2SHORT( 4, sizeof( buf ) ), MPFROMP( buf ) ), 1 );
    check( "  text", strcmp( buf, "4" ) == 0, TRUE );
    mr = WinSendMsg( slider, SLM_QUERYTICKPOS, MPFROMSHORT( 1 ), 0 );
    check( "SLM_QUERYTICKPOS x of tick 1 is 40 right of tick 0",
           SHORT1FROMMR( mr ) - SHORT1FROMMR( WinSendMsg( slider, SLM_QUERYTICKPOS, MPFROMSHORT( 0 ), 0 ) ), 40 );
    detent = (ULONG)WinSendMsg( slider, SLM_ADDDETENT, MPFROMSHORT( 180 ), 0 );
    check( "SLM_ADDDETENT", detent != 0, TRUE );
    mr = WinSendMsg( slider, SLM_QUERYDETENTPOS, MPFROMLONG( detent ), 0 );
    check( "SLM_QUERYDETENTPOS", SHORT1FROMMR( mr ), 180 );
    check( "  on scale 1", SHORT2FROMMR( mr ), SMA_SCALE1 );
    extra = (ULONG)WinSendMsg( slider, SLM_ADDDETENT, MPFROMSHORT( 20 ), 0 );
    check( "SLM_REMOVEDETENT", (LONG)WinSendMsg( slider, SLM_REMOVEDETENT, MPFROMLONG( extra ), 0 ), TRUE );
    check( "SLM_REMOVEDETENT again", (LONG)WinSendMsg( slider, SLM_REMOVEDETENT, MPFROMLONG( extra ), 0 ), FALSE );
    check( "SLM_SETSLIDERINFO increment 2",
           (LONG)WinSendMsg( slider, SLM_SETSLIDERINFO, MPFROM2SHORT( SMA_SLIDERARMPOSITION, SMA_INCREMENTVALUE ),
                             MPFROMSHORT( 2 ) ), TRUE );
    check( "  arm at 80 pels", arm_pos( slider ), 80 );
    check( "SLM_SETSLIDERINFO increment 9 refused",
           (LONG)WinSendMsg( slider, SLM_SETSLIDERINFO, MPFROM2SHORT( SMA_SLIDERARMPOSITION, SMA_INCREMENTVALUE ),
                             MPFROMSHORT( 9 ) ), FALSE );
    WinSendMsg( slider, SLM_SETSLIDERINFO, MPFROM2SHORT( SMA_SLIDERARMPOSITION, SMA_RANGEVALUE ), MPFROMSHORT( 0 ) );
    check( "back home", arm_pos( slider ), 0 );

    /* The vertical slider: scale 2 is primary, its spacing 0 fills the shaft. */
    WinSendMsg( vslider, SLM_SETTICKSIZE, MPFROM2SHORT( SMA_SETALLTICKS, 4 ), 0 );
    check( "vertical: 11 ticks", (LONG)WinSendMsg( vslider, SLM_QUERYTICKSIZE, MPFROMSHORT( 10 ), 0 ), 4 );
    WinSendMsg( vslider, SLM_SETSLIDERINFO, MPFROM2SHORT( SMA_SLIDERARMPOSITION, SMA_INCREMENTVALUE ),
                MPFROMSHORT( 7 ) );
    check( "vertical: increment 7",
           (LONG)WinSendMsg( vslider, SLM_QUERYSLIDERINFO,
                             MPFROM2SHORT( SMA_SLIDERARMPOSITION, SMA_INCREMENTVALUE ), 0 ), 7 );
    mr = WinSendMsg( vslider, SLM_QUERYDETENTPOS, MPFROMLONG( 12345 ), 0 );
    check( "SLM_QUERYDETENTPOS of no detent", (SHORT)SHORT1FROMMR( mr ), SLDERR_INVALID_PARAMETERS );

    /* The read-only progress slider. */
    check( "SLM_SETSLIDERINFO arm dimensions",
           (LONG)WinSendMsg( progress, SLM_SETSLIDERINFO, MPFROM2SHORT( SMA_SLIDERARMDIMENSIONS, 0 ),
                             MPFROM2SHORT( 4, 24 ) ), TRUE );
    WinSendMsg( progress, SLM_SETSLIDERINFO, MPFROM2SHORT( SMA_SLIDERARMPOSITION, SMA_INCREMENTVALUE ),
                MPFROMSHORT( 6 ) );
    mr = WinSendMsg( progress, SLM_QUERYSLIDERINFO, MPFROM2SHORT( SMA_SLIDERARMDIMENSIONS, 0 ), 0 );
    check( "  arm length", SHORT1FROMMR( mr ), 4 );
    check( "  arm breadth", SHORT2FROMMR( mr ), 24 );

    WinSendMsg( ownslider, SLM_SETSLIDERINFO, MPFROM2SHORT( SMA_SLIDERARMPOSITION, SMA_INCREMENTVALUE ),
                MPFROMSHORT( 3 ) );
}

static void check_valuesets( void )
{
    int     i;
    char    buf[16];
    VSTEXT  vt;
    MRESULT mr;

    for( i = 0; i < 8; i++ )
        check( i == 0 ? "VM_SETITEM colour" : "  VM_SETITEM colour",
               (LONG)WinSendMsg( colors, VM_SETITEM, MPFROM2SHORT( i / 4 + 1, i % 4 + 1 ), MPFROMLONG( rgb[i] ) ),
               TRUE );
    check( "VM_QUERYITEM colour",
           (LONG)WinSendMsg( colors, VM_QUERYITEM, MPFROM2SHORT( 2, 2 ), 0 ), 0x00FFFF );
    check( "VM_SETITEM out of the grid",
           (LONG)WinSendMsg( colors, VM_SETITEM, MPFROM2SHORT( 3, 1 ), MPFROMLONG( 0 ) ), FALSE );
    mr = WinSendMsg( colors, VM_QUERYSELECTEDITEM, 0, 0 );
    check( "row 1, column 1 selected at first", SHORT1FROMMR( mr ) == 1 && SHORT2FROMMR( mr ) == 1, TRUE );
    check( "VM_QUERYITEMATTR type from the style",
           (LONG)WinSendMsg( colors, VM_QUERYITEMATTR, MPFROM2SHORT( 1, 1 ), 0 ), VIA_RGB );
    mr = WinSendMsg( colors, VM_QUERYMETRICS, MPFROMSHORT( VMA_ITEMSPACING ), 0 );
    check( "default spacing 0", (LONG)mr, 0 );
    mr = WinSendMsg( colors, VM_QUERYMETRICS, MPFROMSHORT( VMA_ITEMSIZE ), 0 );
    check( "default item size divides the window", SHORT1FROMMR( mr ) > 40 && SHORT1FROMMR( mr ) < 60, TRUE );
    check( "VM_SETMETRICS item size below 2",
           (LONG)WinSendMsg( colors, VM_SETMETRICS, MPFROMSHORT( VMA_ITEMSIZE ), MPFROM2SHORT( 1, 10 ) ), FALSE );

    WinSendMsg( texts, VM_SETITEM, MPFROM2SHORT( 1, 1 ), MPFROMP( "One" ) );
    WinSendMsg( texts, VM_SETITEM, MPFROM2SHORT( 1, 2 ), MPFROMP( "Two" ) );
    WinSendMsg( texts, VM_SETITEM, MPFROM2SHORT( 1, 3 ), MPFROMP( "Three" ) );
    vt.pszItemText = buf;
    vt.ulBufLen = sizeof( buf );
    check( "VM_QUERYITEM text", (LONG)WinSendMsg( texts, VM_QUERYITEM, MPFROM2SHORT( 1, 3 ), MPFROMP( &vt ) ), 5 );
    check( "  the text", strcmp( buf, "Three" ) == 0, TRUE );
    vt.ulBufLen = 0;
    check( "VM_QUERYITEM length only", (LONG)WinSendMsg( texts, VM_QUERYITEM, MPFROM2SHORT( 1, 2 ), MPFROMP( &vt ) ), 3 );
    check( "VM_SETITEMATTR disabled",
           (LONG)WinSendMsg( texts, VM_SETITEMATTR, MPFROM2SHORT( 1, 2 ), MPFROM2SHORT( VIA_DISABLED, TRUE ) ), TRUE );
    check( "VM_SELECTITEM of a disabled item",
           (LONG)WinSendMsg( texts, VM_SELECTITEM, MPFROM2SHORT( 1, 2 ), 0 ), FALSE );
    check( "VM_SELECTITEM", (LONG)WinSendMsg( texts, VM_SELECTITEM, MPFROM2SHORT( 1, 1 ), 0 ), TRUE );

    for( i = 0; i < 4; i++ )
        WinSendMsg( indices, VM_SETITEM, MPFROM2SHORT( 1, i + 1 ), MPFROMLONG( CLR_BLUE + i ) );
    check( "VM_SETITEMATTR owner draw",
           (LONG)WinSendMsg( indices, VM_SETITEMATTR, MPFROM2SHORT( 1, 2 ), MPFROM2SHORT( VIA_OWNERDRAW, TRUE ) ), TRUE );
    check( "  attributes", (LONG)WinSendMsg( indices, VM_QUERYITEMATTR, MPFROM2SHORT( 1, 2 ), 0 ),
           VIA_COLORINDEX | VIA_OWNERDRAW );
    check( "VM_SETITEMATTR two types refused",
           (LONG)WinSendMsg( indices, VM_SETITEMATTR, MPFROM2SHORT( 1, 3 ), MPFROM2SHORT( VIA_RGB | VIA_TEXT, TRUE ) ), FALSE );
}

/* ---------- checks after the session ---------- */

static void check_session( void )
{
    MRESULT mr;

    check( "slider: final position (detent)", arm_pos( slider ), 180 );
    check( "slider: SLN_CHANGE for each move", n_change >= 5, TRUE );
    check( "slider: last SLN_CHANGE", last_change, 180 );
    check( "slider: SLN_SLIDERTRACK while dragged", n_track > 0, TRUE );
    check( "slider: SLN_SETFOCUS", n_setfocus, 1 );
    check( "slider: SLN_KILLFOCUS", n_killfocus, 1 );
    mr = WinSendMsg( colors, VM_QUERYSELECTEDITEM, 0, 0 );
    check( "value set: selected row", SHORT1FROMMR( mr ), 1 );
    check( "  column", SHORT2FROMMR( mr ), 1 );
    check( "value set: VN_SELECT", n_vselect >= 3, TRUE );
    check( "value set: VN_ENTER", n_venter, 1 );
    check( "  VN_ENTER item", last_venter, (ULONG)MPFROM2SHORT( 1, 1 ) );
    check( "value set: VN_SETFOCUS", n_vsetfocus, 1 );
    check( "right-to-left: leftmost is column 3", text_select, (ULONG)MPFROM2SHORT( 1, 3 ) );
    check( "owner-drawn slider: arm drawn by the owner", n_draw_arm > 0, TRUE );
    check( "  other parts offered", n_draw_other >= 2, TRUE );
    check( "  rclItem is the arm", arm_width, 10 );
    check( "owner-drawn value set item", n_draw_item > 0, TRUE );
    check( "  hItem = row 1, column 2", drawn_hitem, (ULONG)MPFROM2SHORT( 1, 2 ) );
    check( "  rclItem width", drawn_item_width > 20, TRUE );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_BORDER;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "SliderValueSetTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "SliderValueSetTest",
                                "PM Slider / Value Set Test", WS_VISIBLE, NULLHANDLE, 0, &client );

    slider    = make_slider( SLS_BUTTONSLEFT | SLS_SNAPTOINCREMENT | SLS_RIBBONSTRIP | WS_TABSTOP,
                             20, 330, 300, 60, ID_SLIDER, 6, 40, 3, 0 );
    vslider   = make_slider( SLS_VERTICAL | SLS_BUTTONSTOP | SLS_PRIMARYSCALE2, 360, 200, 60, 200, ID_VSLIDER,
                             5, 0, 11, 0 );
    progress  = make_slider( SLS_READONLY | SLS_RIBBONSTRIP, 20, 280, 300, 40, ID_PROGRESS, 11, 0, 0, 0 );
    ownslider = make_slider( SLS_OWNERDRAW, 20, 220, 300, 50, ID_OWNSLIDER, 5, 0, 0, 0 );
    colors    = make_valueset( VS_RGB | VS_BORDER | VS_ITEMBORDER | WS_TABSTOP, 20, 40, 240, 100, ID_COLORS, 2, 4 );
    texts     = make_valueset( VS_TEXT | VS_RIGHTTOLEFT, 280, 40, 180, 40, ID_TEXTS, 1, 3 );
    indices   = make_valueset( VS_COLORINDEX, 280, 100, 180, 40, ID_INDICES, 1, 4 );
    check( "WinCreateWindow", slider && vslider && progress && ownslider && colors && texts && indices, TRUE );

    check_sliders();
    check_valuesets();
    /* Notifications of the checks above are still queued: drain them first. */
    while( WinPeekMsg( hab, &qmsg, NULLHANDLE, 0, 0, PM_REMOVE ) ) {
        if( qmsg.msg == WM_QUIT ) break;
        WinDispatchMsg( hab, &qmsg );
    }
    n_change = n_track = n_setfocus = n_killfocus = 0;
    n_vselect = n_venter = n_vsetfocus = 0;
    last_change = -1;
    text_select = 0;

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    check_session();

    p = append_str( line, "pm_slider_valueset_test: done " );
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
