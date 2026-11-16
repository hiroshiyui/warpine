/*
 * pm_dialog_test.c - dialogs and the keyboard under Warpine
 *
 * The main window has a menu and an accelerator table (FCF_MENU |
 * FCF_ACCELTABLE): Ctrl+D opens a modal dialog with an entry field, a list
 * box and OK (default) / Cancel buttons; F3 exits. The integration test
 * drives a session from the keyboard only:
 *
 *   Ctrl+D, type "Warp", Tab, Down, Down, Enter   -> DID_OK, "Warp", item 1;
 *                                                    EN_ / LN_SETFOCUS and
 *                                                    _KILLFOCUS on the way
 *   Ctrl+D, type "ro", Esc                        -> DID_CANCEL; the entry
 *                                                    field was made read-only
 *                                                    and the list emptied
 *   Ctrl+D, Tab, Tab, Right, Enter                -> Cancel (arrow key within
 *                                                    the OK / Cancel group)
 *   Ctrl+D, Tab, Tab, Alt+N, type "Zz", Alt+C     -> the ~Name label focuses the
 *                                                    entry field, ~Cancel cancels
 *   Ctrl+D, type "Hello", Shift+Left twice, type "p!", Shift+Home, Ctrl+Ins,
 *   End, Shift+Ins, Enter                         -> "Help!Help!" (selection
 *                                                    replaced, copied, pasted)
 *   Alt+F4                                        -> WM_SYSCOMMAND(SC_CLOSE),
 *                                                    then WM_CLOSE
 *
 * It also checks WinPeekMsg with filters, that the owner is disabled while
 * the dialog runs, and that WinDlgBox destroys the dialog.
 *
 * Each check is written to stdout:
 *
 *   pm_dialog_test: <name>: ok
 *   pm_dialog_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_dialog_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSFILEMGR
#include <os2.h>
#include <string.h>
#include "pm_dialog_test.h"

static HAB  hab;
static int  n_checks = 0;
static int  n_passed = 0;
static int  round_no = 0;          /* how many times the dialog was opened */
static char name[32];              /* entry field text when OK was pressed */
static LONG selection = -2;        /* list box selection when OK was pressed */
static ULONG name_sel;             /* EM_QUERYSEL of the entry field at OK */
static BOOL saw_sc_close = FALSE;  /* WM_SYSCOMMAND(SC_CLOSE) reached the client */
static HWND frame;                 /* the main window's frame */
static HWND dialog;                /* the dialog of the current round */
static USHORT cancel_source;       /* CMDSRC_* of the last DID_CANCEL */
static BOOL cancel_in_entry;       /* the entry field had the focus at DID_CANCEL */
static char focus_log[16];         /* focus notifications of round 1: E/e = entry
                                      field set/kill focus, L/l = list box */
static BOOL focus_mp2_ok = TRUE;   /* every notification's mp2 was its control */

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
    char  line[128];
    char *p = append_str( line, "pm_dialog_test: " );
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

/* ---------- the dialog ---------- */

MRESULT EXPENTRY NameDlgProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_INITDLG: {
        HWND entry = WinWindowFromID( hwnd, ID_NAME );
        dialog = hwnd;
        WinSendDlgItemMsg( hwnd, ID_LIST, LM_INSERTITEM, MPFROMSHORT( LIT_END ), MPFROMP( "Red" ) );
        WinSendDlgItemMsg( hwnd, ID_LIST, LM_INSERTITEM, MPFROMSHORT( LIT_END ), MPFROMP( "Green" ) );
        WinSendDlgItemMsg( hwnd, ID_LIST, LM_INSERTITEM, MPFROMSHORT( LIT_END ), MPFROMP( "Blue" ) );
        if( round_no == 1 ) {
            /* The first tab stop gets the focus and is named in mp1. */
            check( "WM_INITDLG names the focus control", (LONG)HWNDFROMMP( mp1 ), (LONG)entry );
            check( "the entry field has the focus", (LONG)WinQueryFocus( HWND_DESKTOP ), (LONG)entry );
            check( "controls are children of the dialog",
                   (LONG)WinQueryWindow( entry, QW_PARENT ), (LONG)hwnd );
            check( "the owner is disabled while the dialog runs", WinIsWindowEnabled( frame ), FALSE );
            WinSendDlgItemMsg( hwnd, ID_LIST, LM_INSERTITEM, MPFROMSHORT( 0 ), MPFROMP( "Temp" ) );
            check( "LM_DELETEITEM returns the items left",
                   SHORT1FROMMR( WinSendDlgItemMsg( hwnd, ID_LIST, LM_DELETEITEM, MPFROMSHORT( 0 ), 0 ) ), 3 );
        } else if( round_no == 2 ) {
            check( "LM_DELETEALL", (LONG)WinSendDlgItemMsg( hwnd, ID_LIST, LM_DELETEALL, 0, 0 ), TRUE );
            check( "the list is empty",
                   SHORT1FROMMR( WinSendDlgItemMsg( hwnd, ID_LIST, LM_QUERYITEMCOUNT, 0, 0 ) ), 0 );
            check( "EM_SETREADONLY returns the old state",
                   (LONG)WinSendMsg( entry, EM_SETREADONLY, MPFROMSHORT( TRUE ), 0 ), FALSE );
            check( "EM_QUERYREADONLY", (LONG)WinSendMsg( entry, EM_QUERYREADONLY, 0, 0 ), TRUE );
        }
        return (MRESULT)FALSE;
    }
    case WM_CONTROL:
        if( round_no == 1 && strlen( focus_log ) < sizeof( focus_log ) - 1 ) {
            char *end = focus_log + strlen( focus_log );
            USHORT id = SHORT1FROMMP( mp1 ), code = SHORT2FROMMP( mp1 );
            if( id == ID_NAME && ( code == EN_SETFOCUS || code == EN_KILLFOCUS ) ) {
                *end = code == EN_SETFOCUS ? 'E' : 'e';
            } else if( id == ID_LIST && ( code == LN_SETFOCUS || code == LN_KILLFOCUS ) ) {
                *end = code == LN_SETFOCUS ? 'L' : 'l';
            } else {
                break;
            }
            end[1] = '\0';
            if( HWNDFROMMP( mp2 ) != WinWindowFromID( hwnd, id ) ) focus_mp2_ok = FALSE;
        }
        break;
    case WM_COMMAND:
        if( SHORT1FROMMP( mp1 ) == DID_OK ) {
            /* Enter pressed the default button: a push button command
             * that did not come from the pointer. */
            check( "OK comes from a push button", SHORT1FROMMP( mp2 ), CMDSRC_PUSHBUTTON );
            check( "OK was pressed with the keyboard", SHORT2FROMMP( mp2 ), FALSE );
            if( round_no == 1 ) {
                /* The entry field had the focus from the start; Tab moved it
                 * to the list box, which keeps it while Enter presses OK. */
                check( "focus notifications in order", strcmp( focus_log, "EeL" ), 0 );
                check( "focus notifications carry the control", focus_mp2_ok, TRUE );
            }
            WinQueryDlgItemText( hwnd, ID_NAME, sizeof( name ), name );
            name_sel = (ULONG)WinSendDlgItemMsg( hwnd, ID_NAME, EM_QUERYSEL, 0, 0 );
            selection = (LONG)SHORT1FROMMR( WinSendDlgItemMsg( hwnd, ID_LIST, LM_QUERYSELECTION,
                                                               MPFROMSHORT( LIT_FIRST ), 0 ) );
            WinDismissDlg( hwnd, DID_OK );
            return 0;
        }
        if( SHORT1FROMMP( mp1 ) == DID_CANCEL ) {
            cancel_source = SHORT1FROMMP( mp2 );
            cancel_in_entry = WinQueryFocus( HWND_DESKTOP ) == WinWindowFromID( hwnd, ID_NAME );
            WinQueryDlgItemText( hwnd, ID_NAME, sizeof( name ), name );
        }
        break;   /* DID_CANCEL: WinDefDlgProc dismisses the dialog with it */
    }
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

/* ---------- the main window ---------- */

static void run_dialog( HWND hwnd, MPARAM mp2 )
{
    ULONG result;

    round_no++;
    if( round_no == 1 ) {
        check( "Ctrl+D arrives as an accelerator command", SHORT1FROMMP( mp2 ), CMDSRC_ACCELERATOR );
    }
    result = WinDlgBox( HWND_DESKTOP, hwnd, NameDlgProc, NULLHANDLE, ID_DLG, NULL );
    if( round_no == 1 ) {
        check( "Enter ends the dialog with DID_OK", result, DID_OK );
        check( "the typed name was kept", strcmp( name, "Warp" ), 0 );
        check( "Down twice selects the second item", selection, 1 );
        check( "WinDlgBox destroys the dialog", WinIsWindow( hab, dialog ), FALSE );
        check( "the owner is enabled again", WinIsWindowEnabled( frame ), TRUE );
    } else if( round_no == 2 ) {
        check( "Esc ends the dialog with DID_CANCEL", result, DID_CANCEL );
        check( "text typed into a read-only field is ignored", strlen( name ), 0 );
    } else if( round_no == 3 ) {
        check( "Right then Enter presses Cancel", result, DID_CANCEL );
        check( "Cancel was a push button", cancel_source, CMDSRC_PUSHBUTTON );
    } else if( round_no == 4 ) {
        check( "Alt+C presses ~Cancel", result, DID_CANCEL );
        check( "Alt+N moved the focus to the entry field", cancel_in_entry, TRUE );
        check( "text typed after Alt+N went to the entry field", strcmp( name, "Zz" ), 0 );
    } else if( round_no == 5 ) {
        check( "selected text was replaced, copied and pasted", strcmp( name, "Help!Help!" ), 0 );
        check( "EM_QUERYSEL: no selection, at the end", name_sel, (LONG)MRFROM2SHORT( 10, 10 ) );
    }
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PAINT: {
        RECTL  rcl;
        POINTL pt;
        HPS    hps = WinBeginPaint( hwnd, NULLHANDLE, &rcl );
        GpiErase( hps );
        GpiSetColor( hps, CLR_BLACK );
        pt.x = 10; pt.y = 40;
        GpiCharStringAt( hps, &pt, 41, "Ctrl+D opens the dialog, F3 or Alt+F4 exit" );
        WinEndPaint( hps );
        return 0;
    }
    case WM_COMMAND:
        switch( SHORT1FROMMP( mp1 ) ) {
        case IDM_DIALOG:
            run_dialog( hwnd, mp2 );
            return 0;
        case IDM_EXIT:
            WinPostMsg( hwnd, WM_CLOSE, 0, 0 );
            return 0;
        }
        break;
    case WM_SYSCOMMAND:
        if( SHORT1FROMMP( mp1 ) == SC_CLOSE ) {
            saw_sc_close = TRUE;
            check( "Alt+F4 is the system accelerator for Close",
                   SHORT1FROMMP( mp2 ), CMDSRC_ACCELERATOR );
        }
        break;   /* WinDefWindowProc turns SC_CLOSE into WM_CLOSE */
    case WM_CLOSE:
        check( "WM_CLOSE follows SC_CLOSE", saw_sc_close, TRUE );
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    HWND  client;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER | FCF_MENU | FCF_ACCELTABLE;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "DialogTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "DialogTest",
                                "PM Dialog Test", 0, NULLHANDLE, ID_MAIN, &client );

    check( "the client's parent is the frame", (LONG)WinQueryWindow( client, QW_PARENT ), (LONG)frame );
    check( "the title bar has a height", WinQuerySysValue( HWND_DESKTOP, SV_CYTITLEBAR ) > 0, TRUE );
    check( "the accelerator table loads", WinLoadAccelTable( hab, NULLHANDLE, ID_MAIN ) != NULLHANDLE, TRUE );

    /* WinPeekMsg: a posted message is seen without being removed, then
     * removed; the filters pass only what they name. */
    WinPostMsg( client, WM_USER, MPFROMLONG( 42 ), 0 );
    check( "WinPeekMsg filtered to another message finds nothing",
           WinPeekMsg( hab, &qmsg, client, WM_USER + 1, WM_USER + 1, PM_REMOVE ), FALSE );
    check( "WinPeekMsg PM_NOREMOVE sees the message",
           WinPeekMsg( hab, &qmsg, client, WM_USER, WM_USER, PM_NOREMOVE ) && qmsg.msg == WM_USER, TRUE );
    check( "WinPeekMsg PM_REMOVE takes it",
           WinPeekMsg( hab, &qmsg, client, WM_USER, WM_USER, PM_REMOVE ) && LONGFROMMP( qmsg.mp1 ) == 42, TRUE );
    check( "then it is gone", WinPeekMsg( hab, &qmsg, client, WM_USER, WM_USER, PM_REMOVE ), FALSE );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    check( "the dialog was opened five times", round_no, 5 );
    p = append_str( line, "pm_dialog_test: done " );
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
