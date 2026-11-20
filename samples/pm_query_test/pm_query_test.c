/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_query_test.c - window lookups, input state and system colours
 *
 * Checks the PMWIN calls
 *
 *   - WinMapWindowPoints (points and a RECTL between a control, the client,
 *     the frame and the desktop), WinWindowFromPoint (with and without
 *     fChildren), WinQueryDesktopWindow, WinQueryAnchorBlock, WinIsChild,
 *     WinQueryWindowProcess, WinMultWindowFromIDs, WinEnumDlgItem (tab and
 *     group walks), WinQueryClassName, WinQueryClassInfo
 *   - WinQuerySysColor / WinSetSysColors (the title bar and SYSCLR_WINDOW
 *     change colour; WM_SYSCOLORCHANGE arrives)
 *   - WinSetPointerPos / WinQueryPointerPos, WinQueryMsgTime /
 *     WinQueryMsgPos against the QMSG the loop read
 *   - with input from a GUI script (shift+click in the client, then
 *     Shift+F5): WinGetKeyState as of the message being processed
 *     (VK_BUTTON1, VK_SHIFT, VK_F5), WinQueryMsgPos at the click,
 *     WinGetPhysKeyState of the Shift key
 *
 * The script: "shift+click" somewhere in the client away from the buttons,
 * then "key shift+f5". Each check is written to stdout:
 *
 *   pm_query_test: <name>: ok
 *   pm_query_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_query_test: done <passed>/<total>" after the second
 * release of Shift; the program then ends.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSPROCESS
#include <os2.h>
#include <string.h>

#define CLASS_NAME     "PMQueryTest"
#define CB_WINDOW_DATA 4
#define WM_RUN_CHECKS  ( WM_USER + 1 )
/* Scan code of the left Shift key. */
#define SC_LSHIFT      0x2A
#define N_LINES        8

static HAB   hab;
static HWND  hwndFrame, hwndClient;
static HWND  btn[5];
static QMSG  last_qmsg;
static int   n_checks = 0;
static int   n_passed = 0;
static int   n_syscolorchange = 0;
static ULONG syscolorchange_mp1 = 0xFFFF;
static int   shift_ups = 0;
static char  lines[N_LINES][80];
static int   n_lines = 0;

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

static void note( const char *s )
{
    if( n_lines < N_LINES ) {
        strncpy( lines[n_lines], s, sizeof( lines[0] ) - 1 );
        n_lines++;
    }
}

static void check( const char *what, LONG got, LONG want )
{
    char  line[160];
    char *p = append_str( line, "pm_query_test: " );
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
        note( line + 15 );
    }
    write_stdout( line );
    n_checks++;
}

static void check_error( const char *what, ULONG code )
{
    check( what, ERRORIDERROR( WinGetLastError( hab ) ), code );
}

/* ---------- checks without input ---------- */

static void lookup_checks( void )
{
    LONG      bw = WinQuerySysValue( HWND_DESKTOP, SV_CXSIZEBORDER );
    POINTL    pt;
    RECTL     rcl;
    HWND      ah[6];
    PID       pid;
    TID       tid;
    PTIB      ptib;
    PPIB      ppib;
    CLASSINFO ci;
    char      name[32];

    check( "WinQueryDesktopWindow is HWND_DESKTOP",
           WinQueryDesktopWindow( hab, NULLHANDLE ) == HWND_DESKTOP, TRUE );
    check( "WinQueryAnchorBlock(client)", WinQueryAnchorBlock( hwndClient ) == hab, TRUE );

    /* The frame is at (100, 100); the client lies inside its border. */
    pt.x = 0; pt.y = 0;
    check( "map client -> desktop", WinMapWindowPoints( hwndClient, HWND_DESKTOP, &pt, 1 ), TRUE );
    check( "-> x is frame x + border", pt.x, 100 + bw );
    check( "-> y is frame y", pt.y, 100 );
    pt.x = 0; pt.y = 0;
    WinMapWindowPoints( btn[0], HWND_DESKTOP, &pt, 1 );
    check( "map button -> desktop x", pt.x, 100 + bw + 10 );
    check( "map button -> desktop y", pt.y, 100 + 20 );
    rcl.xLeft = pt.x; rcl.yBottom = pt.y; rcl.xRight = pt.x + 80; rcl.yTop = pt.y + 30;
    check( "map RECTL desktop -> button", WinMapWindowPoints( HWND_DESKTOP, btn[0], (PPOINTL)&rcl, 2 ), TRUE );
    check( "-> (0, 0, 80, 30)", rcl.xLeft == 0 && rcl.yBottom == 0 && rcl.xRight == 80 && rcl.yTop == 30, TRUE );
    pt.x = 0; pt.y = 0;
    WinMapWindowPoints( hwndFrame, hwndClient, &pt, 1 );
    check( "map frame -> client x", pt.x, -bw );

    /* WinWindowFromPoint. */
    pt.x = 100 + bw + 15; pt.y = 100 + 25;
    check( "WinWindowFromPoint(desktop, fChildren) is the button",
           WinWindowFromPoint( HWND_DESKTOP, &pt, TRUE ) == btn[0], TRUE );
    check( "WinWindowFromPoint(desktop, !fChildren) is the frame",
           WinWindowFromPoint( HWND_DESKTOP, &pt, FALSE ) == hwndFrame, TRUE );
    pt.x = 15; pt.y = 25;
    check( "WinWindowFromPoint(client) is the button",
           WinWindowFromPoint( hwndClient, &pt, FALSE ) == btn[0], TRUE );
    pt.x = 300; pt.y = 200;
    check( "a point in no child is the client", WinWindowFromPoint( hwndClient, &pt, TRUE ) == hwndClient, TRUE );
    pt.x = -5; pt.y = 5;
    check( "a point outside is NULLHANDLE", WinWindowFromPoint( hwndClient, &pt, TRUE ), 0 );
    pt.x = 300; pt.y = 100 + 400 - 10;
    check( "the title bar is the frame's", WinWindowFromPoint( HWND_DESKTOP, &pt, TRUE ) == hwndFrame, TRUE );

    /* WinIsChild. */
    check( "WinIsChild(button, frame)", WinIsChild( btn[0], hwndFrame ), TRUE );
    check( "WinIsChild(frame, button)", WinIsChild( hwndFrame, btn[0] ), FALSE );
    check( "WinIsChild(button, button)", WinIsChild( btn[0], btn[0] ), TRUE );
    check( "WinIsChild(frame, HWND_DESKTOP)", WinIsChild( hwndFrame, HWND_DESKTOP ), TRUE );

    /* WinQueryWindowProcess: this process, this (the first) thread. */
    DosGetInfoBlocks( &ptib, &ppib );
    check( "WinQueryWindowProcess", WinQueryWindowProcess( hwndClient, &pid, &tid ), TRUE );
    check( "-> pid", pid, ppib->pib_ulpid );
    check( "-> tid", tid, ptib->tib_ptib2->tib2_ultid );

    /* WinMultWindowFromIDs: ids 101..106, the last has no window. */
    check( "WinMultWindowFromIDs(101..106)", WinMultWindowFromIDs( hwndClient, ah, 101, 106 ), 5 );
    check( "-> [0] is id 101", ah[0] == btn[0], TRUE );
    check( "-> [3] is id 104", ah[3] == btn[3], TRUE );
    check( "-> [5] is NULLHANDLE", ah[5], 0 );

    /* WinEnumDlgItem over the client's buttons: groups [1 2] [3 4 5],
       tab stops 1 and 3 (5 is a disabled tab stop). */
    check( "EDI_FIRSTTABITEM", WinEnumDlgItem( hwndClient, NULLHANDLE, EDI_FIRSTTABITEM ) == btn[0], TRUE );
    check( "EDI_LASTTABITEM", WinEnumDlgItem( hwndClient, NULLHANDLE, EDI_LASTTABITEM ) == btn[2], TRUE );
    check( "EDI_NEXTTABITEM(1)", WinEnumDlgItem( hwndClient, btn[0], EDI_NEXTTABITEM ) == btn[2], TRUE );
    check( "EDI_NEXTTABITEM(3) wraps", WinEnumDlgItem( hwndClient, btn[2], EDI_NEXTTABITEM ) == btn[0], TRUE );
    check( "EDI_PREVTABITEM(4)", WinEnumDlgItem( hwndClient, btn[3], EDI_PREVTABITEM ) == btn[2], TRUE );
    check( "EDI_FIRSTGROUPITEM(2)", WinEnumDlgItem( hwndClient, btn[1], EDI_FIRSTGROUPITEM ) == btn[0], TRUE );
    check( "EDI_NEXTGROUPITEM(2) wraps", WinEnumDlgItem( hwndClient, btn[1], EDI_NEXTGROUPITEM ) == btn[0], TRUE );
    check( "EDI_NEXTGROUPITEM(4) skips the disabled 5",
           WinEnumDlgItem( hwndClient, btn[3], EDI_NEXTGROUPITEM ) == btn[2], TRUE );
    check( "EDI_LASTGROUPITEM(3)", WinEnumDlgItem( hwndClient, btn[2], EDI_LASTGROUPITEM ) == btn[3], TRUE );

    /* WinQueryClassName: "#3" for a button, "#1" for the frame. */
    check( "WinQueryClassName(button)", WinQueryClassName( btn[0], sizeof( name ), name ), 2 );
    check( "-> \"#3\"", strcmp( name, "#3" ), 0 );
    WinQueryClassName( hwndFrame, sizeof( name ), name );
    check( "WinQueryClassName(frame) is \"#1\"", strcmp( name, "#1" ), 0 );
    check( "WinQueryClassName(client)", WinQueryClassName( hwndClient, sizeof( name ), name ), 11 );
    check( "-> the registered name", strcmp( name, CLASS_NAME ), 0 );
    check( "WinQueryClassName cuts to cchMax - 1", WinQueryClassName( hwndClient, 4, name ), 3 );
    check( "-> \"PMQ\"", strcmp( name, "PMQ" ), 0 );

    /* WinQueryClassInfo. */
    check( "WinQueryClassInfo(WC_BUTTON)", WinQueryClassInfo( hab, WC_BUTTON, &ci ), TRUE );
    check( "-> CS_PUBLIC", ( ci.flClassStyle & CS_PUBLIC ) != 0, TRUE );
    check( "-> a window procedure", ci.pfnWindowProc != NULL, TRUE );
    check( "WinQueryClassInfo(\"#1\")", WinQueryClassInfo( hab, "#1", &ci ), TRUE );
    check( "WinQueryClassInfo(own class)", WinQueryClassInfo( hab, CLASS_NAME, &ci ), TRUE );
    check( "-> cbWindowData", ci.cbWindowData, CB_WINDOW_DATA );
    check( "WinQueryClassInfo(unknown)", WinQueryClassInfo( hab, "NoSuchClass", &ci ), FALSE );
    check_error( "-> PMERR_ATOM_NAME_NOT_FOUND", PMERR_ATOM_NAME_NOT_FOUND );
}

static void color_checks( void )
{
    LONG table[4];

    check( "WinQuerySysColor(SYSCLR_WINDOW) is white",
           WinQuerySysColor( HWND_DESKTOP, SYSCLR_WINDOW, 0 ), 0xFFFFFF );
    table[0] = SYSCLR_ACTIVETITLE; table[1] = 0x800000;
    table[2] = SYSCLR_WINDOW;      table[3] = 0xFFFFC0;
    check( "WinSetSysColors(LCOLF_INDRGB)",
           WinSetSysColors( HWND_DESKTOP, 0, LCOLF_INDRGB, 0, 4, table ), TRUE );
    check( "-> SYSCLR_ACTIVETITLE", WinQuerySysColor( HWND_DESKTOP, SYSCLR_ACTIVETITLE, 0 ), 0x800000 );
    check( "-> SYSCLR_WINDOW", WinQuerySysColor( HWND_DESKTOP, SYSCLR_WINDOW, 0 ), 0xFFFFC0 );
    table[0] = 5;
    check( "WinSetSysColors with no system colour index",
           WinSetSysColors( HWND_DESKTOP, 0, LCOLF_INDRGB, 0, 2, table ), FALSE );
    check_error( "-> PMERR_PARAMETER_OUT_OF_RANGE", PMERR_PARAMETER_OUT_OF_RANGE );
    check( "WinQuerySysColor(bad index)", WinQuerySysColor( HWND_DESKTOP, 5, 0 ), 0 );
    check_error( "-> PMERR_PARAMETER_OUT_OF_RANGE", PMERR_PARAMETER_OUT_OF_RANGE );
}

static void pointer_checks( void )
{
    POINTL pt;

    check( "WinSetPointerPos(200, 300)", WinSetPointerPos( HWND_DESKTOP, 200, 300 ), TRUE );
    check( "WinQueryPointerPos", WinQueryPointerPos( HWND_DESKTOP, &pt ), TRUE );
    check( "-> x", pt.x, 200 );
    check( "-> y", pt.y, 300 );
    /* The message being processed is the one the loop read last. */
    check( "WinQueryMsgTime is the QMSG time", WinQueryMsgTime( hab ), last_qmsg.time );
    check( "WinQueryMsgTime is not 0", WinQueryMsgTime( hab ) != 0, TRUE );
    check( "WinQueryMsgPos", WinQueryMsgPos( hab, &pt ), TRUE );
    check( "-> the QMSG ptl", pt.x == last_qmsg.ptl.x && pt.y == last_qmsg.ptl.y, TRUE );
    check( "button 1 is up", WinGetKeyState( HWND_DESKTOP, VK_BUTTON1 ) & 0x8000, 0 );
}

/* ---------- checks driven by the script ---------- */

static void click_checks( HWND hwnd, MPARAM mp1 )
{
    POINTL pt, msg_pt, ptr;

    check( "click: VK_BUTTON1 down", WinGetKeyState( HWND_DESKTOP, VK_BUTTON1 ) & 0x8000, 0x8000 );
    check( "click: VK_SHIFT down", WinGetKeyState( HWND_DESKTOP, VK_SHIFT ) & 0x8000, 0x8000 );
    check( "click: WinQueryMsgPos", WinQueryMsgPos( hab, &msg_pt ), TRUE );
    pt.x = SHORT1FROMMP( mp1 );
    pt.y = SHORT2FROMMP( mp1 );
    WinMapWindowPoints( hwnd, HWND_DESKTOP, &pt, 1 );
    check( "click: message position x is the click", msg_pt.x, pt.x );
    check( "click: message position y is the click", msg_pt.y, pt.y );
    check( "click: the window there is the client",
           WinWindowFromPoint( HWND_DESKTOP, &msg_pt, TRUE ) == hwnd, TRUE );
    WinQueryPointerPos( HWND_DESKTOP, &ptr );
    check( "click: the pointer is there", ptr.x == msg_pt.x && ptr.y == msg_pt.y, TRUE );
}

static void finish( void )
{
    char line[64];

    check( "WM_SYSCOLORCHANGE received", n_syscolorchange >= 1, TRUE );
    check( "-> mp1 is flOptions", syscolorchange_mp1, 0 );
    check( "Shift up, pressed twice", WinGetKeyState( HWND_DESKTOP, VK_SHIFT ), 0 );
    check( "WinGetPhysKeyState(Shift): up, pressed", WinGetPhysKeyState( HWND_DESKTOP, SC_LSHIFT ), 0x0001 );
    check( "WinGetPhysKeyState again: pressed bit cleared", WinGetPhysKeyState( HWND_DESKTOP, SC_LSHIFT ), 0 );
    strcpy( line, "pm_query_test: done " );
    append_long( line + strlen( line ), n_passed );
    strcat( line, "/" );
    append_long( line + strlen( line ), n_checks );
    strcat( line, "\r\n" );
    write_stdout( line );
    WinPostMsg( hwndClient, WM_QUIT, 0, 0 );
}

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_RUN_CHECKS:
        lookup_checks();
        color_checks();
        pointer_checks();
        WinInvalidateRect( hwnd, NULL, FALSE );
        return 0;
    case WM_SYSCOLORCHANGE:
        n_syscolorchange++;
        syscolorchange_mp1 = (ULONG)mp1;
        return 0;
    case WM_BUTTON1DOWN:
        click_checks( hwnd, mp1 );
        return (MRESULT)TRUE;
    case WM_BUTTON1UP:
        check( "release: VK_BUTTON1 up, pressed once",
               WinGetKeyState( HWND_DESKTOP, VK_BUTTON1 ), 0x0001 );
        return (MRESULT)TRUE;
    case WM_CHAR:
    {
        USHORT fs = SHORT1FROMMP( mp1 );
        USHORT vk = SHORT2FROMMP( mp2 );
        if( !( fs & KC_VIRTUALKEY ) ) break;
        if( vk == VK_F5 && !( fs & KC_KEYUP ) ) {
            check( "F5: VK_F5 down", WinGetKeyState( HWND_DESKTOP, VK_F5 ) & 0x8000, 0x8000 );
            check( "F5: VK_SHIFT down", WinGetKeyState( HWND_DESKTOP, VK_SHIFT ) & 0x8000, 0x8000 );
        } else if( vk == VK_SHIFT && ( fs & KC_KEYUP ) ) {
            if( ++shift_ups == 2 ) finish();
        }
        return (MRESULT)TRUE;
    }
    case WM_PAINT:
    {
        RECTL rcl, line;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        int   i;
        char  text[40];
        WinQueryWindowRect( hwnd, &rcl );
        /* SYSCLR_WINDOW: light yellow once WinSetSysColors has run. */
        WinFillRect( hps, &rcl, SYSCLR_WINDOW );
        WinSetRect( hab, &line, 8, rcl.yTop - 24, rcl.xRight - 8, rcl.yTop - 8 );
        strcpy( text, "checks passed " );
        append_long( text + strlen( text ), n_passed );
        strcat( text, "/" );
        append_long( text + strlen( text ), n_checks );
        WinDrawText( hps, -1, text, &line, CLR_BLACK, SYSCLR_WINDOW, DT_LEFT | DT_TOP );
        for( i = 0; i < n_lines; i++ ) {
            WinOffsetRect( hab, &line, 0, -16 );
            WinDrawText( hps, -1, lines[i], &line, CLR_RED, SYSCLR_WINDOW, DT_LEFT | DT_TOP );
        }
        WinEndPaint( hps );
        return 0;
    }
    case WM_CLOSE:
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    ULONG flags = FCF_TITLEBAR | FCF_SIZEBORDER | FCF_SYSMENU | FCF_MINMAX;
    static const ULONG styles[5] = {
        WS_GROUP | WS_TABSTOP, 0, WS_GROUP | WS_TABSTOP, 0, WS_TABSTOP | WS_DISABLED
    };
    static const char *labels[5] = { "One", "Two", "Three", "Four", "Five" };
    int   i;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, CLASS_NAME, ClientWndProc, CS_SIZEREDRAW, CB_WINDOW_DATA );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, CLASS_NAME,
                                    "Lookups, input state, system colours", 0,
                                    NULLHANDLE, 0, &hwndClient );
    WinSetWindowPos( hwndFrame, HWND_TOP, 100, 100, 500, 400,
                     SWP_MOVE | SWP_SIZE | SWP_ACTIVATE | SWP_SHOW );
    for( i = 0; i < 5; i++ ) {
        btn[i] = WinCreateWindow( hwndClient, WC_BUTTON, labels[i],
                                  WS_VISIBLE | BS_PUSHBUTTON | styles[i],
                                  10 + 90 * i, 20, 80, 30, hwndClient, HWND_TOP,
                                  101 + i, NULL, NULL );
    }
    WinPostMsg( hwndClient, WM_RUN_CHECKS, 0, 0 );

    while( WinGetMsg( hab, &last_qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &last_qmsg );

    WinDestroyWindow( hwndFrame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
