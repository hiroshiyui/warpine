/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_sysmenu_test.c - a frame's system menu, keyboard move / size and a
 * pop-up menu under Warpine
 *
 * The integration test drives one frame (minimize and maximize buttons,
 * sizing border) with the mouse and the keyboard. The client logs every
 * WM_SYSCOMMAND it receives and checks, at each one, what the step before
 * did:
 *
 *   1  click the system-menu icon, then "m"  SC_MOVE from the menu (keyboard)
 *      Right Right Down Enter                the frame moved 16 right, 8 down
 *   2  Alt+F8                                SC_SIZE from an accelerator
 *      Right Right Down Down Esc             size picks edges; Esc restores
 *   3  Alt+F7                                SC_MOVE; the frame is as before
 *      Left Enter
 *   4  Shift+Esc                             SC_SYSMENU; moved 8 left
 *   5  "x" in the menu                       SC_MAXIMIZE, which the client
 *                                            intercepts (no WinDefWindowProc)
 *   6  Alt+Space                             SC_SYSMENU; not maximized
 *   7  "s" in the menu, Right Right Enter    SC_SIZE; 8 pels wider (WM_SIZE)
 *      right click in the client            WinPopupMenu (PU_POSITIONONITEM
 *      click on the item under the pointer   on Green): WM_COMMAND(Green)
 *   8  Alt+Space, click on Close             SC_CLOSE with fPointer TRUE
 *
 * then WinDefWindowProc posts WM_CLOSE and the program ends. Each check is
 * written to stdout:
 *
 *   pm_sysmenu_test: <name>: ok
 *   pm_sysmenu_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_sysmenu_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_DOSPROCESS
#include <os2.h>
#include <string.h>

#define ID_POPUP    100
#define IDM_GREEN   302

static HAB  hab;
static HWND frame, client, popup;
static int  n_checks = 0;
static int  n_passed = 0;
static int  n_sys = 0;              /* WM_SYSCOMMAND received */
static SWP  start, sized;           /* frame position at steps 1 and 2 */
static LONG client_cx = 0;          /* from the last WM_SIZE */
static LONG sized_client_cx = 0;    /* client width at step 2 */

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
    p = append_str( line, "pm_sysmenu_test: " );
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

static SWP pos( void )
{
    SWP swp;
    WinQueryWindowPos( frame, &swp );
    return swp;
}

static LONG client_width( void )
{
    RECTL rc;
    WinQueryWindowRect( client, &rc );
    return rc.xRight - rc.xLeft;
}

/* Check the command, its source and whether it came from the mouse. */
static void check_command( const char *step, MPARAM mp1, MPARAM mp2, USHORT cmd, USHORT src, BOOL ptr )
{
    char name[80], *p;
    p = append_str( name, step );
    append_str( p, ": command" );
    check( name, SHORT1FROMMP( mp1 ), cmd );
    p = append_str( name, step );
    append_str( p, ": source" );
    check( name, SHORT1FROMMP( mp2 ), src );
    p = append_str( name, step );
    append_str( p, ": fPointer" );
    check( name, SHORT2FROMMP( mp2 ) != 0, ptr );
}

/* ---------- the client ---------- */

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    SWP now;

    switch( msg ) {
    case WM_SYSCOMMAND:
        n_sys++;
        now = pos();
        switch( n_sys ) {
        case 1:
            check_command( "1 menu Move", mp1, mp2, SC_MOVE, CMDSRC_MENU, FALSE );
            start = now;
            break;
        case 2:
            check_command( "2 Alt+F8", mp1, mp2, SC_SIZE, CMDSRC_ACCELERATOR, FALSE );
            check( "1 keyboard move: x", now.x, start.x + 16 );
            check( "1 keyboard move: y", now.y, start.y - 8 );
            check( "1 keyboard move: size kept", now.cx, start.cx );
            sized = now;
            sized_client_cx = client_width();
            break;
        case 3:
            check_command( "3 Alt+F7", mp1, mp2, SC_MOVE, CMDSRC_ACCELERATOR, FALSE );
            check( "2 Esc restores the size: cx", now.cx, sized.cx );
            check( "2 Esc restores the size: cy", now.cy, sized.cy );
            check( "2 Esc restores the size: y", now.y, sized.y );
            break;
        case 4:
            check_command( "4 Shift+Esc", mp1, mp2, SC_SYSMENU, CMDSRC_ACCELERATOR, FALSE );
            check( "3 keyboard move left: x", now.x, sized.x - 8 );
            break;
        case 5:
            /* Intercepted: the frame must not be maximized. */
            check_command( "5 menu Maximize", mp1, mp2, SC_MAXIMIZE, CMDSRC_MENU, FALSE );
            return 0;
        case 6:
            check_command( "6 Alt+Space", mp1, mp2, SC_SYSMENU, CMDSRC_ACCELERATOR, FALSE );
            check( "5 intercepted Maximize: not maximized",
                   ( WinQueryWindowULong( frame, QWL_STYLE ) & WS_MAXIMIZED ) != 0, FALSE );
            break;
        case 7:
            check_command( "7 menu Size", mp1, mp2, SC_SIZE, CMDSRC_MENU, FALSE );
            break;
        case 8:
            check_command( "8 Alt+Space", mp1, mp2, SC_SYSMENU, CMDSRC_ACCELERATOR, FALSE );
            break;
        case 9:
            check_command( "9 clicked Close", mp1, mp2, SC_CLOSE, CMDSRC_MENU, TRUE );
            check( "7 keyboard size: 8 pels wider", now.cx, sized.cx + 8 );
            check( "7 keyboard size: WM_SIZE with the new width", client_cx, sized_client_cx + 8 );
            check( "7 keyboard size: client 8 pels wider", client_width(), sized_client_cx + 8 );
            break;
        }
        return WinDefWindowProc( hwnd, msg, mp1, mp2 );

    case WM_BUTTON2DOWN:
        check( "WinPopupMenu", WinPopupMenu( hwnd, hwnd, popup, SHORT1FROMMP( mp1 ), SHORT2FROMMP( mp1 ),
                                             IDM_GREEN, PU_POSITIONONITEM | PU_HCONSTRAIN | PU_VCONSTRAIN |
                                             PU_MOUSEBUTTON1 | PU_KEYBOARD ), TRUE );
        return (MRESULT)TRUE;

    case WM_COMMAND:
        check_command( "pop-up menu Green", mp1, mp2, IDM_GREEN, CMDSRC_MENU, TRUE );
        return 0;

    case WM_SIZE:
        client_cx = SHORT1FROMMP( mp2 );
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

int main( void )
{
    HMQ   hmq;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MINBUTTON | FCF_MAXBUTTON;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "SysMenuTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "SysMenuTest",
                                "PM System Menu Test", 0, NULLHANDLE, 0, &client );
    popup = WinLoadMenu( client, NULLHANDLE, ID_POPUP );
    check( "WinLoadMenu of the pop-up menu", popup != NULLHANDLE, TRUE );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    check( "every step reached", n_sys, 9 );
    p = append_str( line, "pm_sysmenu_test: done " );
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
