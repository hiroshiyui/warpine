/*
 * pm_button_test.c - check boxes, radio buttons and the arrow keys under Warpine
 *
 * A dialog has an auto check box (Bold), an auto three-state button
 * (Italic), a check box the dialog procedure sets itself (Underline), a
 * group of auto-radio buttons (Left, Centre, Right; Right has
 * BS_NOCURSORSELECT) and a group of radio buttons the dialog procedure sets
 * (Small, Large). The integration test drives it:
 *
 *   Space                -> Bold checked
 *   Tab, Space, Space    -> Italic checked, then indeterminate
 *   Tab, Space           -> Underline: BN_CLICKED, the procedure checks it
 *   Tab, Space           -> Tab enters the radio group on Centre, the
 *                           button checked with BM_SETCHECK in WM_INITDLG
 *   Down                 -> focus on Right, Centre stays checked
 *   Down                 -> wraps to Left, which the cursor checks
 *   Tab, Space           -> Large (checked by the procedure) has the tab stop
 *   Up, Space            -> Up only moves the focus to Small; Space selects it
 *   Alt+B                -> the ~Bold mnemonic unchecks Bold
 *   click on Right       -> the pointer checks Right, Left is unchecked
 *   Enter                -> OK: the states are checked
 *
 * Each check is written to stdout:
 *
 *   pm_button_test: <name>: ok
 *   pm_button_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_button_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#define INCL_DOSFILEMGR
#include <os2.h>
#include <string.h>
#include "pm_button_test.h"

static int  n_checks = 0;
static int  n_passed = 0;
static char clicks[32];            /* BN_CLICKED in order, one letter per button */
static BOOL focus_on_centre;       /* Centre had the focus when it was selected */
static BOOL owner_set_ok = TRUE;   /* BN_CLICKED of a plain button left its state */

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
    char *p = append_str( line, "pm_button_test: " );
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

static LONG query_check( HWND hwnd, USHORT id )
{
    return (LONG)WinSendDlgItemMsg( hwnd, id, BM_QUERYCHECK, 0, 0 );
}

static LONG check_index( HWND hwnd, USHORT id )
{
    return (LONG)(SHORT)SHORT1FROMMR( WinSendDlgItemMsg( hwnd, id, BM_QUERYCHECKINDEX, 0, 0 ) );
}

static void log_click( HWND hwnd, USHORT id )
{
    static const char letters[] = "BIULCRSG";
    size_t n = strlen( clicks );
    if( id >= ID_BOLD && id <= ID_LARGE && n < sizeof( clicks ) - 1 ) {
        clicks[n] = letters[id - ID_BOLD];
        clicks[n + 1] = '\0';
    }
    switch( id ) {
    case ID_CENTRE:
        focus_on_centre = WinQueryFocus( HWND_DESKTOP ) == WinWindowFromID( hwnd, ID_CENTRE );
        break;
    case ID_UNDER:
        /* A plain check box does not change by itself: the owner sets it. */
        if( query_check( hwnd, ID_UNDER ) != 0 ) owner_set_ok = FALSE;
        WinSendDlgItemMsg( hwnd, ID_UNDER, BM_SETCHECK, MPFROMSHORT( 1 ), 0 );
        break;
    case ID_SMALL:
    case ID_LARGE:
        if( query_check( hwnd, id ) != ( id == ID_LARGE ) ) owner_set_ok = FALSE;
        WinSendDlgItemMsg( hwnd, ID_SMALL, BM_SETCHECK, MPFROMSHORT( id == ID_SMALL ), 0 );
        WinSendDlgItemMsg( hwnd, ID_LARGE, BM_SETCHECK, MPFROMSHORT( id == ID_LARGE ), 0 );
        break;
    }
}

MRESULT EXPENTRY ButtonDlgProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_INITDLG:
        check( "the first tab stop has the focus",
               (LONG)WinQueryFocus( HWND_DESKTOP ), (LONG)WinWindowFromID( hwnd, ID_BOLD ) );
        check( "no radio button is checked yet", check_index( hwnd, ID_LEFT ), -1 );
        check( "BM_SETCHECK returns the old state",
               (LONG)WinSendDlgItemMsg( hwnd, ID_CENTRE, BM_SETCHECK, MPFROMSHORT( 1 ), 0 ), 0 );
        check( "BM_SETCHECK again returns checked",
               (LONG)WinSendDlgItemMsg( hwnd, ID_CENTRE, BM_SETCHECK, MPFROMSHORT( 1 ), 0 ), 1 );
        check( "BM_QUERYCHECKINDEX from another button of the group", check_index( hwnd, ID_RIGHT ), 1 );
        check( "BM_QUERYCHECKINDEX of a check box", check_index( hwnd, ID_BOLD ), -1 );
        check( "BM_QUERYCHECK of a push button", query_check( hwnd, DID_OK ), 0 );
        WinCheckButton( hwnd, ID_LARGE, 1 );
        check( "WinQueryButtonCheckstate", WinQueryButtonCheckstate( hwnd, ID_LARGE ), 1 );
        return (MRESULT)FALSE;
    case WM_CONTROL:
        if( SHORT2FROMMP( mp1 ) == BN_CLICKED ) {
            check( "BN_CLICKED carries the button",
                   (LONG)HWNDFROMMP( mp2 ), (LONG)WinWindowFromID( hwnd, SHORT1FROMMP( mp1 ) ) );
            log_click( hwnd, SHORT1FROMMP( mp1 ) );
        }
        return 0;
    case WM_COMMAND:
        if( SHORT1FROMMP( mp1 ) == DID_OK ) {
            check( "BN_CLICKED in order", strcmp( clicks, "BIIUCLGSBR" ), 0 );
            check( "Tab entered the group on the checked button", focus_on_centre, TRUE );
            check( "plain buttons wait for their owner", owner_set_ok, TRUE );
            check( "Bold: Space checked, Alt+B unchecked", query_check( hwnd, ID_BOLD ), 0 );
            check( "Italic: two presses make it indeterminate", query_check( hwnd, ID_ITALIC ), 2 );
            check( "Underline: set by the owner", query_check( hwnd, ID_UNDER ), 1 );
            check( "Left: unchecked by the click on Right", query_check( hwnd, ID_LEFT ), 0 );
            check( "Centre: unchecked when the cursor checked Left", query_check( hwnd, ID_CENTRE ), 0 );
            check( "Right: checked by the click", query_check( hwnd, ID_RIGHT ), 1 );
            check( "BM_QUERYCHECKINDEX of the alignment group", check_index( hwnd, ID_LEFT ), 2 );
            check( "Small: selected with Space", query_check( hwnd, ID_SMALL ), 1 );
            check( "Large: unchecked by the owner", query_check( hwnd, ID_LARGE ), 0 );
            check( "BM_QUERYCHECKINDEX of the size group", check_index( hwnd, ID_LARGE ), 0 );
        }
        break;
    }
    return WinDefDlgProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HAB   hab;
    HMQ   hmq;
    ULONG result;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );

    result = WinDlgBox( HWND_DESKTOP, HWND_DESKTOP, ButtonDlgProc, NULLHANDLE, ID_DLG, NULL );
    check( "Enter ends the dialog with DID_OK", result, DID_OK );

    p = append_str( line, "pm_button_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
