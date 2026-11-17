/*
 * pm_typeahead_test.c - type-ahead routing under Warpine
 *
 * PM routes keyboard input when the application reads it, not when the key
 * is pressed: keys typed while the program is busy go to the window that
 * has the focus once the program gets to them.
 *
 * The client has the focus at first. When it gets the character 'n' it is
 * busy for a while (DosSleep), then gives the focus to the first entry
 * field. The integration test types, during that pause:
 *
 *   n         -> the client (it moves the focus on)
 *   ab        -> the first entry field: typed before the focus moved, but
 *                read after
 *   click on the second entry field, then cd -> the second entry field
 *
 * A timer then checks the result. Each check is written to stdout:
 *
 *   pm_typeahead_test: <name>: ok
 *   pm_typeahead_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_typeahead_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#define INCL_DOSPROCESS
#include <os2.h>
#include <string.h>

#define ID_TIMER    1
#define ID_ENTRY1   101
#define ID_ENTRY2   102
#define WM_START    ( WM_USER + 1 )
#define BUSY_MS     600
#define CHECK_MS    2500

static HAB  hab;
static HWND entry1, entry2;
static int  n_checks = 0;
static int  n_passed = 0;
static int  client_chars = 0;   /* characters the client got */

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

static void report( const char *what, BOOL ok, const char *got, const char *want )
{
    char  line[160];
    char *p = append_str( line, "pm_typeahead_test: " );
    p = append_str( p, what );
    if( ok ) {
        append_str( p, ": ok\r\n" );
        n_passed++;
    } else {
        p = append_str( p, ": FAIL got " );
        p = append_str( p, got );
        p = append_str( p, " want " );
        p = append_str( p, want );
        append_str( p, "\r\n" );
    }
    write_stdout( line );
    n_checks++;
}

static void check( const char *what, LONG got, LONG want )
{
    char g[12], w[12];
    append_long( g, got );
    append_long( w, want );
    report( what, got == want, g, w );
}

static void check_text( const char *what, HWND hwnd, const char *want )
{
    char text[64];
    WinQueryWindowText( hwnd, sizeof( text ), text );
    report( what, strcmp( text, want ) == 0, text, want );
}

/* ---------- the client ---------- */

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_START:
        entry1 = WinCreateWindow( hwnd, WC_ENTRYFIELD, "", WS_VISIBLE | ES_MARGIN,
                                  20, 380, 200, 24, hwnd, HWND_TOP, ID_ENTRY1, NULL, NULL );
        entry2 = WinCreateWindow( hwnd, WC_ENTRYFIELD, "", WS_VISIBLE | ES_MARGIN,
                                  20, 330, 200, 24, hwnd, HWND_TOP, ID_ENTRY2, NULL, NULL );
        WinStartTimer( hab, hwnd, ID_TIMER, CHECK_MS );
        return 0;

    case WM_CHAR: {
        USHORT fs = SHORT1FROMMP( mp1 );
        if( ( fs & KC_CHAR ) && !( fs & KC_KEYUP ) ) {
            client_chars++;
            if( SHORT1FROMMP( mp2 ) == 'n' ) {
                /* Busy: the keys typed meanwhile wait in the input queue. */
                DosSleep( BUSY_MS );
                WinSetFocus( HWND_DESKTOP, entry1 );
            }
        }
        return (MRESULT)TRUE;
    }

    case WM_TIMER:
        WinStopTimer( hab, hwnd, ID_TIMER );
        check( "only 'n' reached the client", client_chars, 1 );
        check_text( "keys typed ahead go to the new focus window", entry1, "ab" );
        check_text( "keys typed after a click go to the clicked field", entry2, "cd" );
        check( "the clicked field has the focus", WinQueryFocus( HWND_DESKTOP ) == entry2, TRUE );
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
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
    HWND  frame, client;
    QMSG  qmsg;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "TypeAheadTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "TypeAheadTest",
                                "PM Type-ahead Test", 0, NULLHANDLE, 0, &client );
    WinPostMsg( client, WM_START, 0, 0 );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    p = append_str( line, "pm_typeahead_test: done " );
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
