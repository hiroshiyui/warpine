/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_subclass_test.c - painting, subclassing, frame formatting, dialog
 * items of the application's own class, and a thread's windows
 *
 * A standard frame at desktop (100, 200), 400 x 300, with a client. The
 * program checks, on its own:
 *
 *   WinUpdateWindow paints at once, and a WinBeginPaint presentation space
 *   is clipped to the update rectangle (only the invalidated square turns
 *   blue);
 *   a child window's presentation space (WinGetPS) draws at the child's
 *   place in the client;
 *   WinShowWindow invalidates a window it shows;
 *   WinSubclassWindow on an entry field returns the built-in procedure
 *   (also reported by WinQueryClassInfo), which the subclass passes
 *   messages on to: a WM_CHAR typed through the subclass reaches the field;
 *   a subclassed frame answers WM_QUERYFRAMECTLCOUNT and WM_FORMATFRAME
 *   when it is sized, and the tool bar it places above the client stays
 *   there;
 *   built-in controls answer WM_QUERYDLGCODE and WM_MATCHMNEMONIC;
 *   the windows of a thread that ends without destroying them go with it.
 *
 * Then a dialog holds an entry field, a "Dial" control (this program's own
 * class) and an OK button. The integration test types
 *
 *   Alt+Z   no built-in control has Z: WinDefDlgProc asks the Dial
 *           (WM_MATCHMNEMONIC), which says TRUE and takes the focus;
 *   Tab     the Dial's WinDefWindowProc passes the key to its owner, the
 *           dialog, which moves on to OK;
 *   Shift+Tab  back to the Dial;
 *   Alt+O   OK's mnemonic, through the Dial and the dialog: OK is pressed.
 *
 * Each check is written to stdout:
 *
 *   pm_subclass_test: <name>: ok
 *   pm_subclass_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_subclass_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOS
#include <os2.h>
#include <string.h>
#include "pm_subclass_test.h"

#define TOOL_H 24

static HAB   hab;
static HWND  frame, client, kid, field, toolbar;
static int   n_checks = 0;
static int   n_passed = 0;
static LONG  paint_color = CLR_YELLOW;
static int   n_client_paint = 0, n_kid_paint = 0;
static PFNWP old_field, old_frame;
static int   n_field_char = 0, n_count = 0, n_format = 0;
static int   n_match = 0, n_dial_focus = 0;
static USHORT last_match = 0;
static HWND  thread_frame = NULLHANDLE;

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
    char *p = append_str( append_str( line, "pm_subclass_test: " ), what );
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

/* The colour index of client pel (x, y). */
static LONG client_pel( LONG x, LONG y )
{
    POINTL pt;
    HPS    hps = WinGetPS( client );
    LONG   c;
    pt.x = x; pt.y = y;
    c = GpiQueryPel( hps, &pt );
    WinReleasePS( hps );
    return c;
}

static MRESULT EXPENTRY ClientProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_PAINT ) {
        RECTL r;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        WinQueryWindowRect( hwnd, &r );
        WinFillRect( hps, &r, paint_color );
        WinEndPaint( hps );
        n_client_paint++;
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static MRESULT EXPENTRY KidProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_PAINT ) {
        RECTL r;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        WinQueryWindowRect( hwnd, &r );
        WinFillRect( hps, &r, CLR_GREEN );
        WinEndPaint( hps );
        n_kid_paint++;
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

/* The entry field's subclass: counts characters, passes everything on. */
static MRESULT EXPENTRY FieldProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_CHAR ) n_field_char++;
    return old_field( hwnd, msg, mp1, mp2 );
}

/* The frame's subclass: one more frame control, a tool bar above the client. */
static MRESULT EXPENTRY FrameProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_QUERYFRAMECTLCOUNT ) {
        n_count++;
        return (MRESULT)( (ULONG)old_frame( hwnd, msg, mp1, mp2 ) + 1 );
    }
    if( msg == WM_FORMATFRAME ) {
        PSWP  swp = (PSWP)mp1;
        ULONG n = (ULONG)old_frame( hwnd, msg, mp1, mp2 );
        ULONG i;
        n_format++;
        for( i = 0; i < n; i++ ) {
            if( swp[i].hwnd == client ) {
                swp[i].cy -= TOOL_H;
                swp[n].fl = SWP_SIZE | SWP_MOVE | SWP_SHOW;
                swp[n].x = swp[i].x;
                swp[n].y = swp[i].y + swp[i].cy;
                swp[n].cx = swp[i].cx;
                swp[n].cy = TOOL_H;
                swp[n].hwnd = toolbar;
                swp[n].hwndInsertBehind = HWND_TOP;
                return (MRESULT)( n + 1 );
            }
        }
        return (MRESULT)n;
    }
    return old_frame( hwnd, msg, mp1, mp2 );
}

/* The dialog's own control: Z is its mnemonic. */
static MRESULT EXPENTRY DialProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_MATCHMNEMONIC: {
        USHORT c = SHORT1FROMMP( mp1 );
        n_match++;
        last_match = c;
        return (MRESULT)( c == 'z' || c == 'Z' );
    }
    case WM_SETFOCUS:
        if( SHORT1FROMMP( mp2 ) ) n_dial_focus++;
        return 0;
    case WM_PAINT: {
        RECTL r;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        WinQueryWindowRect( hwnd, &r );
        WinFillRect( hps, &r, CLR_CYAN );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static void dialog_checks( HWND dlg )
{
    check( "the Dial was asked one mnemonic", n_match, 1 );
    check( "... Z", last_match == 'z' || last_match == 'Z', TRUE );
    check( "the Dial took the focus twice (Alt+Z, Shift+Tab)", n_dial_focus, 2 );
    check( "OK was pressed through the Dial's owner", TRUE, TRUE );
    (void)dlg;
}

static MRESULT EXPENTRY DlgProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_COMMAND && SHORT1FROMMP( mp1 ) == DID_OK ) {
        dialog_checks( hwnd );
        WinDestroyWindow( hwnd );
        WinPostMsg( client, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

static void APIENTRY ThreadFn( ULONG arg )
{
    HAB   h = WinInitialize( 0 );
    HMQ   q = WinCreateMsgQueue( h, 0 );
    ULONG fl = FCF_TITLEBAR;
    HWND  c;
    (void)arg;
    (void)q;
    thread_frame = WinCreateStdWindow( HWND_DESKTOP, 0, &fl, "SubTest", "Thread", 0, NULLHANDLE, 0, &c );
    /* Ends without WinDestroyWindow or WinDestroyMsgQueue. */
}

static void run_checks( void )
{
    RECTL     r, rc0, rc1;
    CLASSINFO ci;
    SWP       swp;
    TID       tid;
    char      text[16];
    HWND      dlg, name, ok;
    int       before;

    /* WinUpdateWindow paints at once; the WinBeginPaint PS is clipped. */
    paint_color = CLR_YELLOW;
    WinInvalidateRect( client, NULL, FALSE );
    before = n_client_paint;
    WinUpdateWindow( client );
    check( "WinUpdateWindow paints before it returns", n_client_paint - before, 1 );
    check( "the client is yellow", client_pel( 300, 100 ), CLR_YELLOW );
    paint_color = CLR_BLUE;
    r.xLeft = 200; r.yBottom = 150; r.xRight = 220; r.yTop = 170;
    WinInvalidateRect( client, &r, FALSE );
    WinUpdateWindow( client );
    check( "the update rectangle is painted", client_pel( 210, 160 ), CLR_BLUE );
    check( "the rest is clipped away", client_pel( 300, 100 ), CLR_YELLOW );

    /* A child window's PS draws at its place. */
    {
        HPS   hps = WinGetPS( kid );
        RECTL kr;
        WinQueryWindowRect( kid, &kr );
        WinFillRect( hps, &kr, CLR_RED );
        WinReleasePS( hps );
    }
    check( "the child's PS drew at the child", client_pel( 25, 35 ), CLR_RED );
    check( "... not at the client's origin", client_pel( 5, 5 ), CLR_YELLOW );

    /* WinShowWindow invalidates what it shows. */
    WinShowWindow( kid, FALSE );
    before = n_kid_paint;
    WinShowWindow( kid, TRUE );
    WinUpdateWindow( kid );
    check( "a shown window is painted", n_kid_paint - before, 1 );

    /* Subclassing a built-in control. */
    old_field = WinSubclassWindow( field, FieldProc );
    check( "WinSubclassWindow gives the built-in procedure", old_field != NULL, TRUE );
    check( "WinQueryClassInfo(WC_ENTRYFIELD)", WinQueryClassInfo( hab, WC_ENTRYFIELD, &ci ), TRUE );
    check( "... reports the same procedure", ci.pfnWindowProc == old_field, TRUE );
    WinSetFocus( HWND_DESKTOP, field );
    WinSendMsg( field, WM_CHAR, MPFROMSH2CH( KC_CHAR, 1, 0 ), MPFROM2SHORT( 'A', 0 ) );
    check( "the subclass saw WM_CHAR", n_field_char, 1 );
    WinQueryWindowText( field, sizeof( text ), text );
    check( "... and passed it on to the field", strcmp( text, "A" ), 0 );

    /* Built-in controls answer the dialog's questions. */
    check( "WM_QUERYDLGCODE of an entry field",
           (LONG)WinSendMsg( field, WM_QUERYDLGCODE, 0, 0 ) & DLGC_ENTRYFIELD, DLGC_ENTRYFIELD );

    /* A subclassed frame formats its controls. */
    WinQueryWindowRect( client, &rc0 );
    old_frame = WinSubclassWindow( frame, FrameProc );
    check( "a standard frame has the built-in procedure", old_frame != NULL, TRUE );
    WinSetWindowPos( frame, HWND_TOP, 0, 0, 400, 320, SWP_SIZE );
    check( "WM_QUERYFRAMECTLCOUNT was asked", n_count >= 1, TRUE );
    check( "WM_FORMATFRAME was asked", n_format >= 1, TRUE );
    WinQueryWindowRect( client, &rc1 );
    check( "the client gave room to the tool bar", rc1.yTop - rc0.yTop, 20 - TOOL_H );
    WinQueryWindowPos( toolbar, &swp );
    check( "the tool bar is 24 pels high", swp.cy, TOOL_H );
    check( "... and visible", WinIsWindowVisible( toolbar ), TRUE );

    /* A thread's windows end with it. */
    DosCreateThread( &tid, ThreadFn, 0, 0, 65536 );
    DosWaitThread( &tid, DCWW_WAIT );
    check( "the thread made a window", thread_frame != NULLHANDLE, TRUE );
    check( "... which went with the thread", WinIsWindow( hab, thread_frame ), FALSE );

    /* The dialog with a control of this program's class. */
    dlg = WinLoadDlg( HWND_DESKTOP, frame, DlgProc, NULLHANDLE, ID_DLG, NULL );
    check( "the dialog is loaded", dlg != NULLHANDLE, TRUE );
    name = WinWindowFromID( dlg, ID_NAME );
    ok = WinWindowFromID( dlg, DID_OK );
    check( "WM_MATCHMNEMONIC: ~OK matches o", (LONG)WinSendMsg( ok, WM_MATCHMNEMONIC, MPFROMSHORT( 'o' ), 0 ), TRUE );
    check( "... not z", (LONG)WinSendMsg( ok, WM_MATCHMNEMONIC, MPFROMSHORT( 'z' ), 0 ), FALSE );
    WinSetFocus( HWND_DESKTOP, name );
}

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[48];

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "SubTest", ClientProc, CS_SIZEREDRAW, 0 );
    WinRegisterClass( hab, "Kid", KidProc, 0, 0 );
    WinRegisterClass( hab, "Dial", DialProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &flags, "SubTest",
                                "Subclassing", WS_VISIBLE, NULLHANDLE, 0, &client );
    WinSetWindowPos( frame, HWND_TOP, 100, 200, 400, 300, SWP_MOVE | SWP_SIZE | SWP_SHOW | SWP_ACTIVATE );
    kid = WinCreateWindow( client, "Kid", "", WS_VISIBLE, 20, 30, 60, 40, client, HWND_TOP, 10, NULL, NULL );
    field = WinCreateWindow( client, WC_ENTRYFIELD, "", WS_VISIBLE | ES_MARGIN, 150, 30, 120, 20,
                             client, HWND_TOP, 11, NULL, NULL );
    toolbar = WinCreateWindow( frame, WC_STATIC, "Tools", SS_TEXT | DT_LEFT | DT_VCENTER | WS_VISIBLE,
                               0, 0, 10, 10, frame, HWND_TOP, 0x7F00, NULL, NULL );
    run_checks();

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) ) WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( frame );
    append_str( append_long( append_str( append_long( append_str( line, "pm_subclass_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
