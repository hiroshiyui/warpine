/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_menukey_test.c - the menu bar from the keyboard under Warpine
 *
 * A frame with a menu bar (File: New, a separator, a disabled item, Open;
 * Edit: Copy, Paste). The integration test drives it with keys only:
 *
 *   F10, Right, Down, Down, Enter   -> Edit / Paste    (WM_COMMAND 202)
 *   F1                              -> WM_HELP to the client
 *   F10, f, Down, Enter             -> File / Open     (WM_COMMAND 102;
 *                                      Down skips the separator and the
 *                                      disabled item)
 *   F10, Esc, then the letter x     -> x reaches the client as WM_CHAR
 *   Alt+F4                          -> closes the window
 *
 * In menu mode the menu takes the keys: the client must see no character
 * but the x. The client also hears the menu: WM_INITMENU for the bar and
 * for the Edit pull-down before it drops, WM_MENUSELECT with the post flag
 * for each chosen item (its TRUE reply lets the command be posted), and
 * WM_MENUEND each time menu mode ends. main() also sends MM_STARTMENUMODE / MM_ENDMENUMODE to the
 * FID_MENU window. Each check is written to stdout:
 *
 *   pm_menukey_test: <name>: ok
 *   pm_menukey_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_menukey_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_DOS
#include <os2.h>
#include <string.h>
#include "pm_menukey_test.h"

#define MAX_LOG 16

static int   n_checks = 0;
static int   n_passed = 0;
static ULONG commands[MAX_LOG];
static int   n_commands = 0;
static int   menu_sources = 0;     /* WM_COMMANDs with CMDSRC_MENU, fPointer FALSE */
static int   helps = 0;
static int   init_bar = 0, init_edit = 0, chosen_paste = 0, chosen_open = 0, ends = 0;
static char  chars[MAX_LOG + 1];
static int   n_chars = 0;

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
    char  line[200];
    char *p = append_str( line, "pm_menukey_test: " );
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
    n_checks++;
    write_stdout( line );
}

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_COMMAND:
        if( n_commands < MAX_LOG )
            commands[n_commands++] = SHORT1FROMMP( mp1 );
        if( SHORT1FROMMP( mp2 ) == CMDSRC_MENU && SHORT2FROMMP( mp2 ) == FALSE )
            menu_sources++;
        return 0;
    case WM_INITMENU:
        if( SHORT1FROMMP( mp1 ) == FID_MENU ) init_bar++;
        if( SHORT1FROMMP( mp1 ) == IDM_EDIT ) init_edit++;
        break;
    case WM_MENUSELECT:
        if( SHORT2FROMMP( mp1 ) && SHORT1FROMMP( mp1 ) == IDM_PASTE ) chosen_paste++;
        if( SHORT2FROMMP( mp1 ) && SHORT1FROMMP( mp1 ) == IDM_OPEN ) chosen_open++;
        break;   /* WinDefWindowProc returns TRUE: post the command */
    case WM_MENUEND:
        ends++;
        break;
    case WM_HELP:
        helps++;
        return 0;
    case WM_CHAR:
        if( ( SHORT1FROMMP( mp1 ) & ( KC_CHAR | KC_KEYUP ) ) == KC_CHAR && n_chars < MAX_LOG )
            chars[n_chars++] = (char)SHORT1FROMMP( mp2 );
        break;
    case WM_PAINT: {
        RECTL r;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &r );
        WinFillRect( hps, &r, CLR_WHITE );
        WinEndPaint( hps );
        return 0;
    }
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HAB   hab;
    HMQ   hmq;
    QMSG  qmsg;
    HWND  frame, client, menu;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_MENU | FCF_BORDER;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "MenuKeyTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "MenuKeyTest",
                                "PM Menu Key Test", WS_VISIBLE, NULLHANDLE, ID_MAIN, &client );
    menu = WinWindowFromID( frame, FID_MENU );
    check( "the frame has a menu bar", menu != NULLHANDLE, TRUE );
    check( "MM_STARTMENUMODE", (LONG)WinSendMsg( menu, MM_STARTMENUMODE, MPFROMSHORT( TRUE ), 0 ), TRUE );
    WinSendMsg( menu, MM_ENDMENUMODE, MPFROMSHORT( TRUE ), 0 );
    WinSetFocus( HWND_DESKTOP, client );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    chars[n_chars] = '\0';
    check( "two commands chosen", n_commands, 2 );
    check( "  Edit / Paste", commands[0], IDM_PASTE );
    check( "  File / Open (the separator and disabled item skipped)", commands[1], IDM_OPEN );
    check( "  both from the menu, by keyboard", menu_sources, 2 );
    check( "F1 posts WM_HELP", helps, 1 );
    check( "WM_INITMENU for the menu bar", init_bar >= 3, TRUE );
    check( "WM_INITMENU for Edit before it dropped", init_edit, 1 );
    check( "WM_MENUSELECT(TRUE) for Paste", chosen_paste, 1 );
    check( "WM_MENUSELECT(TRUE) for Open", chosen_open, 1 );
    check( "WM_MENUEND whenever menu mode ended", ends >= 3, TRUE );
    check( "the client saw one character", n_chars, 1 );
    check( "  the x typed after Esc", chars[0], 'x' );

    p = append_str( line, "pm_menukey_test: done " );
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
