/*
 * pm_list_test.c - list box selection modes and the MLE under Warpine
 *
 * The client holds three list boxes of eight items each - one with
 * LS_MULTIPLESEL, one with LS_EXTENDEDSEL and one that selects a single
 * item - and an MLE with MLS_WORDWRAP. At creation it checks the messages:
 * LM_SELECTITEM / LM_QUERYSELECTION on a multiple-selection list (several
 * items, walked from LIT_FIRST; LIT_NONE clears) and MLM_QUERYWRAP /
 * MLM_SETWRAP.
 *
 * The integration test then drives it with the mouse and keyboard:
 *
 *   click rows 0 and 2 of the multiple list      -> items 0 and 2 selected
 *   Down, Space in it                             -> the cursor (row 3) is
 *                                                    toggled on as well
 *   click row 1 of the extended list, Shift+click
 *   row 3, Ctrl+click row 2                       -> items 1 and 3 selected
 *   double-click row 4 of the single list, Enter  -> two LN_ENTERs, item 4
 *   F3                                            -> final checks, exit
 *
 * After its message loop it checks the queue lifecycle: a second
 * WinCreateMsgQueue fails, WinDestroyMsgQueue succeeds once, and the thread
 * can then create a new queue.
 *
 * Each check is written to stdout:
 *
 *   pm_list_test: <name>: ok
 *   pm_list_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_list_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#include <os2.h>
#include <string.h>

#define ID_MULTI   101
#define ID_EXT     102
#define ID_SINGLE  103
#define ID_MLE     104

static HAB  hab;
static int  n_checks = 0;
static int  n_passed = 0;
static HWND multi, ext, single, mle;
static int  single_enters = 0;     /* LN_ENTER from the single list */
static LONG enter_selection = -2;  /* its selection at the last LN_ENTER */
static int  other_enters = 0;      /* LN_ENTER from the other lists */

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
    char *p = append_str( line, "pm_list_test: " );
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

/* The selected items of a list box as a bit mask (bit n = item n), walked
 * with LM_QUERYSELECTION as applications do; 0 when nothing is selected. */
#define BIT( n ) ( 1L << ( n ) )
static LONG selected_items( HWND list )
{
    LONG item = LIT_FIRST, mask = 0;
    for( ;; ) {
        item = (LONG)WinSendMsg( list, LM_QUERYSELECTION, MPFROMSHORT( item ), 0 );
        if( item == LIT_NONE || item < 0 || item > 30 ) break;
        mask |= BIT( item );
    }
    return mask;
}

static HWND make_list( HWND parent, ULONG style, LONG x, ULONG id )
{
    static const char *items[] = {
        "Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel"
    };
    HWND list = WinCreateWindow( parent, WC_LISTBOX, NULL, WS_VISIBLE | style,
                                 x, 250, 180, 150, parent, HWND_TOP, id, NULL, NULL );
    int i;
    for( i = 0; i < 8; i++ )
        WinSendMsg( list, LM_INSERTITEM, MPFROMSHORT( LIT_END ), MPFROMP( items[i] ) );
    return list;
}

/* ---------- the client ---------- */

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_CREATE:
        multi  = make_list( hwnd, LS_MULTIPLESEL, 10,  ID_MULTI );
        ext    = make_list( hwnd, LS_EXTENDEDSEL, 210, ID_EXT );
        single = make_list( hwnd, 0,              410, ID_SINGLE );
        mle = WinCreateWindow( hwnd, WC_MLE,
                               "Word wrap breaks long lines at the spaces between words.",
                               WS_VISIBLE | MLS_WORDWRAP, 10, 100, 180, 100,
                               hwnd, HWND_TOP, ID_MLE, NULL, NULL );

        /* A multiple-selection list keeps every item selected. */
        WinSendMsg( multi, LM_SELECTITEM, MPFROMSHORT( 5 ), MPFROMSHORT( TRUE ) );
        WinSendMsg( multi, LM_SELECTITEM, MPFROMSHORT( 7 ), MPFROMSHORT( TRUE ) );
        check( "LM_SELECTITEM adds to a multiple selection", selected_items( multi ), BIT( 5 ) | BIT( 7 ) );
        WinSendMsg( multi, LM_SELECTITEM, MPFROMSHORT( 5 ), MPFROMSHORT( FALSE ) );
        check( "LM_SELECTITEM FALSE removes one item", selected_items( multi ), BIT( 7 ) );
        check( "LM_SELECTITEM LIT_NONE clears the selection",
               (LONG)WinSendMsg( multi, LM_SELECTITEM, MPFROMSHORT( LIT_NONE ), 0 ), TRUE );
        check( "nothing is selected", selected_items( multi ), 0 );
        /* A single-selection list replaces it. */
        WinSendMsg( single, LM_SELECTITEM, MPFROMSHORT( 1 ), MPFROMSHORT( TRUE ) );
        WinSendMsg( single, LM_SELECTITEM, MPFROMSHORT( 2 ), MPFROMSHORT( TRUE ) );
        check( "a single-selection list keeps one item", selected_items( single ), BIT( 2 ) );
        WinSendMsg( single, LM_SELECTITEM, MPFROMSHORT( LIT_NONE ), 0 );

        check( "MLS_WORDWRAP turns word wrap on",
               (LONG)WinSendMsg( mle, MLM_QUERYWRAP, 0, 0 ), TRUE );
        WinSendMsg( mle, MLM_SETWRAP, MPFROMSHORT( FALSE ), 0 );
        check( "MLM_SETWRAP FALSE turns it off", (LONG)WinSendMsg( mle, MLM_QUERYWRAP, 0, 0 ), FALSE );
        WinSendMsg( mle, MLM_SETWRAP, MPFROMSHORT( TRUE ), 0 );
        return 0;

    case WM_CONTROL:
        if( SHORT2FROMMP( mp1 ) == LN_ENTER ) {
            if( SHORT1FROMMP( mp1 ) == ID_SINGLE ) {
                single_enters++;
                enter_selection = selected_items( single );
            } else {
                other_enters++;
            }
        }
        return 0;

    case WM_CHAR:
        if( !( SHORT1FROMMP( mp1 ) & KC_KEYUP ) && ( SHORT1FROMMP( mp1 ) & KC_VIRTUALKEY )
            && SHORT2FROMMP( mp2 ) == VK_F3 ) {
            check( "clicks toggle items of a multiple list, Space the cursor item",
                   selected_items( multi ), BIT( 0 ) | BIT( 2 ) | BIT( 3 ) );
            check( "LIT_CURSOR names the cursor item",
                   (LONG)WinSendMsg( multi, LM_QUERYSELECTION, MPFROMSHORT( LIT_CURSOR ), 0 ), 3 );
            check( "Shift+click and Ctrl+click in an extended list", selected_items( ext ), BIT( 1 ) | BIT( 3 ) );
            check( "a double-click and Enter send LN_ENTER", single_enters, 2 );
            check( "the double-clicked item is selected", enter_selection, BIT( 4 ) );
            check( "no other list sent LN_ENTER", other_enters, 0 );
            WinPostMsg( hwnd, WM_QUIT, 0, 0 );
            return (MRESULT)TRUE;
        }
        break;

    case WM_PAINT: {
        RECTL rc;
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
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
    HWND  frame, client;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "ListTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "ListTest",
                                "PM List Test", 0, NULLHANDLE, 0, &client );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    /* One message queue per thread; it can be destroyed once and then
     * created again. */
    check( "a second WinCreateMsgQueue fails", (LONG)WinCreateMsgQueue( hab, 0 ), (LONG)NULLHANDLE );
    WinDestroyWindow( frame );
    check( "WinDestroyMsgQueue destroys the queue", WinDestroyMsgQueue( hmq ), TRUE );
    check( "a destroyed queue cannot be destroyed again", WinDestroyMsgQueue( hmq ), FALSE );
    hmq = WinCreateMsgQueue( hab, 0 );
    check( "then the thread can create a new queue", hmq != NULLHANDLE, TRUE );

    p = append_str( line, "pm_list_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
