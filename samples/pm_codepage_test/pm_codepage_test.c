/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_codepage_test.c - message-queue and presentation-space codepages
 *
 * Checks, from the guest side, that PM text follows the codepage of the
 * message queue (WinSetCp) rather than the process codepage:
 *
 *   - WinQueryCpList lists the queue codepages, including 1004;
 *   - WinSetCp(hmq, 1004) is accepted, DosSetProcessCp(1004) is not;
 *     WinSetCp refuses an unknown codepage and 0 (PMERR_RESOURCE_NOT_FOUND)
 *     and an unknown queue (PMERR_INVALID_HMQ);
 *   - window text set and read back on a CP1004 queue keeps CP1004 bytes
 *     (0x05 is a breve there, a control character elsewhere), and read on a
 *     CP852 queue it comes back as CP852's breve;
 *   - a presentation space created afterwards starts in CP1004
 *     (GpiQueryCp), and GpiSetCp changes it.
 *
 * Output, one line per check, then a summary:
 *
 *   pm_codepage_test: <name>: ok
 *   pm_codepage_test: <name>: FAIL got <value> want <value>
 *   pm_codepage_test: done <passed>/<total>
 */
#define INCL_WIN
#define INCL_WINCOUNTRY
#define INCL_GPI
#define INCL_DOSNLS
#define INCL_DOSFILEMGR
#define INCL_WINERRORS
#include <os2.h>
#include <string.h>

#define CP_WINDOWS_EXTENDED 1004
#define MAX_CPS             64

static int n_checks = 0;
static int n_passed = 0;

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
    char *p = append_str( line, "pm_codepage_test: " );
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

static BOOL in_list( const ULONG *cps, ULONG n, ULONG cp )
{
    ULONG i;
    for( i = 0; i < n; i++ ) {
        if( cps[i] == cp ) return TRUE;
    }
    return FALSE;
}

int main( void )
{
    HAB   hab;
    HMQ   hmq;
    HWND  frame, client;
    HPS   hps;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU;
    ULONG cps[MAX_CPS];
    ULONG n, process_cp, cb;
    /* "Br" + breve + "ve" in CP1004 (0x05 = U+02D8 BREVE). */
    static const char breve_text[] = "Br\x05ve";
    char  back[32];
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "CodepageTest", WinDefWindowProc, 0, 0 );

    DosQueryCp( sizeof( process_cp ), &process_cp, &cb );

    n = WinQueryCpList( hab, MAX_CPS, cps );
    check( "WinQueryCpList returns some codepages", n > 0, TRUE );
    check( "the list includes 850", in_list( cps, n, 850 ), TRUE );
    check( "the list includes 1004", in_list( cps, n, CP_WINDOWS_EXTENDED ), TRUE );
    check( "WinQueryCpList with no buffer returns the count", WinQueryCpList( hab, 0, NULL ), n );

    check( "DosSetProcessCp refuses 1004", DosSetProcessCp( CP_WINDOWS_EXTENDED ) != 0, TRUE );
    check( "WinSetCp accepts 1004", WinSetCp( hmq, CP_WINDOWS_EXTENDED ), TRUE );
    check( "WinQueryCp reports 1004", WinQueryCp( hmq ), CP_WINDOWS_EXTENDED );
    check( "the process codepage is unchanged", ( DosQueryCp( sizeof( n ), &n, &cb ), n ), process_cp );
    check( "WinSetCp refuses an unknown codepage", WinSetCp( hmq, 12345 ), FALSE );
    check( "-> PMERR_RESOURCE_NOT_FOUND", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_RESOURCE_NOT_FOUND );
    check( "WinSetCp refuses 0", WinSetCp( hmq, 0 ), FALSE );
    check( "-> PMERR_RESOURCE_NOT_FOUND (0)", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_RESOURCE_NOT_FOUND );
    check( "-> the queue keeps 1004", WinQueryCp( hmq ), CP_WINDOWS_EXTENDED );
    check( "WinSetCp refuses an unknown queue", WinSetCp( (HMQ)0x7777, 850 ), FALSE );
    check( "-> PMERR_INVALID_HMQ", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INVALID_HMQ );

    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "CodepageTest",
                                "Codepage test", 0, NULLHANDLE, 0, &client );
    WinSetWindowText( frame, (PSZ)breve_text );
    memset( back, 0, sizeof( back ) );
    check( "window text comes back with its length",
           WinQueryWindowText( frame, sizeof( back ), back ), (LONG)strlen( breve_text ) );
    check( "and the same CP1004 bytes", memcmp( back, breve_text, sizeof( breve_text ) ), 0 );
    /* The text is stored as characters, not bytes: on a CP852 queue the
     * breve reads back as CP852's breve, 0xF4. */
    WinSetCp( hmq, 852 );
    WinQueryWindowText( frame, sizeof( back ), back );
    check( "read on a CP852 queue the breve is 0xF4", (UCHAR)back[2], 0xF4 );
    WinSetCp( hmq, CP_WINDOWS_EXTENDED );

    hps = WinGetPS( client );
    check( "a new PS uses the queue codepage", GpiQueryCp( hps ), CP_WINDOWS_EXTENDED );
    check( "GpiSetCp accepts 850", GpiSetCp( hps, 850 ), TRUE );
    check( "GpiQueryCp reports it", GpiQueryCp( hps ), 850 );
    WinReleasePS( hps );

    p = append_str( line, "pm_codepage_test: done " );
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
