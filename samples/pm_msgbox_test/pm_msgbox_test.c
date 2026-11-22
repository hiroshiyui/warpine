/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_msgbox_test.c - WinMessageBox as a modal PM dialog under Warpine
 *
 * After start-up the client opens five message boxes, one after another,
 * owned by its frame. The integration test answers them:
 *
 *   MB_YESNOCANCEL | MB_DEFBUTTON2, Enter   -> MBID_NO (the default button)
 *   MB_OKCANCEL, Esc                        -> MBID_CANCEL
 *   MB_YESNO, click on Yes                  -> MBID_YES
 *   MB_OK with two lines of text, Enter     -> MBID_OK
 *   MB_YESNO | MB_HELP | MB_MOVEABLE, click on Help, then
 *     a click on the close button           -> MBID_CANCEL
 *   WinMessageBox2, buttons Keep / Discard
 *     (BS_DEFAULT), a custom icon, Enter    -> 502 (Discard)
 *   WinMessageBox2 MB_NONMODAL, Enter       -> returns its handle at once;
 *     WM_MSGBOXINIT, then WM_MSGBOXDISMISS with the button's id reach the
 *     client, which destroys the box
 *
 * The boxes show the four icons (question, none, warning, information,
 * error). Help sends WM_HELP to the message box itself, which stays open;
 * the box's parent is the desktop, so the owner's client never sees it.
 * Close ends a box with MBID_CANCEL although it has no Cancel button;
 * only a box with MB_MOVEABLE has the system menu and the close button.
 *
 * A timer keeps running on the client while the boxes are open: the
 * owner's window procedure gets its WM_TIMERs during the modal loop, and
 * sees its frame disabled.
 *
 * Each check is written to stdout:
 *
 *   pm_msgbox_test: <name>: ok
 *   pm_msgbox_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_msgbox_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#include <os2.h>
#include <string.h>

#define ID_TIMER   1
#define WM_START   ( WM_USER + 1 )

static HAB  hab;
static HWND frame;
static int  n_checks = 0;
static int  n_passed = 0;
static int  ticks_while_open = 0;   /* WM_TIMERs during a message box */
static int  box_open = 0;
static BOOL owner_disabled = TRUE;  /* the frame was disabled at every such tick */
static int  help_seen = 0;          /* WM_HELPs that reached the client */

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
    char *p = append_str( line, "pm_msgbox_test: " );
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

/* An MB2INFO with two buttons. */
typedef struct { MB2INFO info; MB2D more[1]; } MB2INFO2;

static ULONG box2( HWND owner, ULONG style, HWND notify, ULONG id )
{
    MB2INFO2 mb;
    memset( &mb, 0, sizeof( mb ) );
    mb.info.cb = sizeof( mb );
    mb.info.hIcon = WinQuerySysPointer( HWND_DESKTOP, SPTR_ICONWARNING, FALSE );
    mb.info.cButtons = 2;
    mb.info.flStyle = style;
    mb.info.hwndNotify = notify;
    strcpy( mb.info.mb2d[0].achText, "~Keep" );
    mb.info.mb2d[0].idButton = 501;
    strcpy( mb.more[0].achText, "~Discard" );
    mb.more[0].idButton = 502;
    mb.more[0].flStyle = BS_DEFAULT;
    return WinMessageBox2( HWND_DESKTOP, owner, "Keep the draft?", "Message Box 2", id, &mb.info );
}

static HWND  nonmodal = NULLHANDLE;
static int   inits = 0;
static ULONG dismissed_with = 0;

static ULONG box( HWND owner, PSZ text, ULONG style )
{
    ULONG rc;
    box_open = 1;
    rc = WinMessageBox( HWND_DESKTOP, owner, text, "Message Box Test", 0, style );
    box_open = 0;
    return rc;
}

/* ---------- the client ---------- */

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_START:
        WinStartTimer( hab, hwnd, ID_TIMER, 100 );
        check( "Enter picks the default button (MB_DEFBUTTON2)",
               box( hwnd, "Save the changes?", MB_YESNOCANCEL | MB_DEFBUTTON2 | MB_ICONQUESTION ), MBID_NO );
        check( "Esc cancels", box( hwnd, "Continue?", MB_OKCANCEL ), MBID_CANCEL );
        check( "a click on Yes", box( hwnd, "Delete the file?", MB_YESNO | MB_ICONEXCLAMATION ), MBID_YES );
        check( "Enter on OK",
               box( hwnd, "The operation completed.\nTwo lines of text in one box.", MB_OK | MB_INFORMATION ),
               MBID_OK );
        check( "Close without a Cancel button",
               box( hwnd, "Delete the file anyway?", MB_YESNO | MB_HELP | MB_ERROR | MB_MOVEABLE ), MBID_CANCEL );
        check( "WM_HELP stays with the message box", help_seen, 0 );
        WinStopTimer( hab, hwnd, ID_TIMER );
        check( "the owner's timer ran while a box was open", ticks_while_open > 0, TRUE );
        check( "the owner was disabled while a box was open", owner_disabled, TRUE );
        check( "and is enabled again", WinIsWindowEnabled( frame ), TRUE );
        check( "WinMessageBox2: Enter picks the BS_DEFAULT button",
               box2( hwnd, MB_CUSTOMICON | MB_MOVEABLE, NULLHANDLE, 0 ), 502 );
        check( "WinMessageBox2 with an unknown flag", box2( hwnd, 0x0001, NULLHANDLE, 0 ), MBID_ERROR );
        check( "  -> PMERR_INVALID_FLAG", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INVALID_FLAG );
        nonmodal = (HWND)box2( hwnd, MB_NONMODAL, hwnd, 33 );
        check( "a non-modal box returns its handle at once", WinIsWindow( hab, nonmodal ), TRUE );
        check( "  and leaves its owner enabled", WinIsWindowEnabled( frame ), TRUE );
        return 0;

    case WM_MSGBOXINIT:
        if( (HWND)mp1 == nonmodal && LONGFROMMP( mp2 ) == 33 ) inits++;
        return 0;

    case WM_MSGBOXDISMISS:
        dismissed_with = LONGFROMMP( mp2 );
        check( "WM_MSGBOXINIT came first, with the box and its identity", inits, 1 );
        check( "WM_MSGBOXDISMISS: the box", (HWND)mp1 == nonmodal, TRUE );
        check( "  and Enter's default button", dismissed_with, 502 );
        check( "the owner destroys the box", WinDestroyWindow( nonmodal ), TRUE );
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;

    case WM_HELP:
        help_seen++;
        return 0;

    case WM_TIMER:
        if( box_open ) {
            ticks_while_open++;
            if( WinIsWindowEnabled( frame ) ) owner_disabled = FALSE;
        }
        return 0;

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
    HWND  client;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "MsgBoxTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "MsgBoxTest",
                                "PM Message Box Test", 0, NULLHANDLE, 0, &client );
    WinPostMsg( client, WM_START, 0, 0 );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    p = append_str( line, "pm_msgbox_test: done " );
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
