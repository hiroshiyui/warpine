/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_profile_test.c - PM profiles (PMSHAPI Prf*) and the Help Manager
 * (HELPMGR) under Warpine
 *
 * Profiles:
 *   - PrfWriteProfileString / PrfQueryProfileString in the user profile,
 *     HINI_PROFILE searching it, the system profile not having it
 *   - the default string for a missing key (PMERR_NOT_IN_IDX recorded),
 *     truncation to the buffer (still NUL-terminated)
 *   - PrfQueryProfileInt ("-42xyz" -> -42, text -> 0, missing -> default)
 *   - PrfQueryProfileSize (a string counts its NUL; name lists)
 *   - enumeration: NULL application lists applications, NULL key lists
 *     keys, as NUL-separated names ending in a double NUL
 *   - PrfWriteProfileData / PrfQueryProfileData with binary data and a
 *     short buffer
 *   - NULL data deletes a key, a NULL key deletes the application
 *   - PrfOpenProfile of a private file (same handle when opened again),
 *     PrfCloseProfile, bad handles (PMERR_INVALID_INI_FILE_HANDLE), and
 *     PMERR_INI_FILE_IS_SYS_OR_USER for OS2.INI
 *   - PrfQueryProfile: the lengths the names need, the names, a short
 *     buffer (cut, PMERR_BUFFER_TOO_SMALL)
 *   - PrfReset: a wrong system profile refused, a new user profile
 *     (C:\NEWUSER.INI, made) that HINI_USERPROFILE then reads and writes,
 *     PL_ALTERED posted to the queue, and back to OS2.INI
 *
 * Help Manager:
 *   - WinCreateHelpInstance returns a handle and clears ulReturnCode
 *   - WinAssociateHelpInstance / WinQueryHelpInstance (the client finds
 *     its frame's instance), WinLoadHelpTable, WinCreateHelpTable,
 *     HM_* messages sent to the instance, disassociation, and
 *     WinDestroyHelpInstance
 *
 * The user profile is C:\OS2\OS2.INI, the private one C:\PRFTEST.INI;
 * the program leaves the application "PrfTest" in the user profile and
 * "Private" in C:\PRFTEST.INI for the caller to inspect.
 *
 * Each check is written to stdout:
 *
 *   pm_profile_test: <name>: ok
 *   pm_profile_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_profile_test: done <passed>/<total>" when the program ends.
 */
#define INCL_WIN
#define INCL_WINSHELLDATA
#define INCL_WINHELP
#define INCL_SHLERRORS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define APP       "PrfTest"
#define TEMP_APP  "PrfTemp"
#define PRIVATE   "C:\\PRFTEST.INI"
#define BAD_HINI  ( (HINI)0x4321 )
#define HELP_TABLE_ID 1000

static int n_checks = 0;
static int n_passed = 0;

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
    char  line[200];
    char *p = append_str( line, "pm_profile_test: " );
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

/* Whether the first n bytes of a and b are equal. */
static BOOL same( const void *a, const void *b, ULONG n )
{
    return memcmp( a, b, n ) == 0;
}

static void check_error( HAB hab, const char *what, USHORT code )
{
    check( what, ERRORIDERROR( WinGetLastError( hab ) ), code );
}

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

static void profile_checks( HAB hab )
{
    char   buf[64];
    ULONG  n, size;
    HINI   hini, again;
    static const BYTE blob[6] = { 0, 1, 2, 0xFF, '"', 0 };

    /* Strings in the user profile. */
    PrfWriteProfileData( HINI_USERPROFILE, TEMP_APP, NULL, NULL, 0 );
    check( "PrfWriteProfileString", PrfWriteProfileString( HINI_USERPROFILE, APP, "Greeting", "hello" ), TRUE );
    memset( buf, 0x55, sizeof( buf ) );
    n = PrfQueryProfileString( HINI_USERPROFILE, APP, "Greeting", NULL, buf, sizeof( buf ) );
    check( "PrfQueryProfileString returns length with NUL", n, 6 );
    check( "-> the string", same( buf, "hello", 6 ), TRUE );
    n = PrfQueryProfileString( HINI_PROFILE, APP, "Greeting", NULL, buf, sizeof( buf ) );
    check( "HINI_PROFILE finds the user profile's key", n, 6 );
    n = PrfQueryProfileString( HINI_SYSTEMPROFILE, APP, "Greeting", NULL, buf, sizeof( buf ) );
    check( "the system profile does not have it", n, 0 );
    check_error( hab, "-> PMERR_NOT_IN_IDX", PMERR_NOT_IN_IDX );

    n = PrfQueryProfileString( HINI_PROFILE, APP, "Missing", "dflt", buf, sizeof( buf ) );
    check( "missing key returns the default", n, 5 );
    check( "-> default text", same( buf, "dflt", 5 ), TRUE );
    check_error( hab, "-> PMERR_NOT_IN_IDX", PMERR_NOT_IN_IDX );
    check( "missing key without a default returns 0",
           PrfQueryProfileString( HINI_PROFILE, APP, "Missing", NULL, buf, sizeof( buf ) ), 0 );

    memset( buf, 0x55, sizeof( buf ) );
    n = PrfQueryProfileString( HINI_USERPROFILE, APP, "Greeting", NULL, buf, 4 );
    check( "a short buffer gets what fits", n, 4 );
    check( "-> NUL-terminated", same( buf, "hel", 4 ), TRUE );
    check( "-> nothing written past it", (BYTE)buf[4], 0x55 );

    /* Integers. */
    PrfWriteProfileString( HINI_USERPROFILE, APP, "Number", "-42xyz" );
    PrfWriteProfileString( HINI_USERPROFILE, APP, "Text", "text" );
    check( "PrfQueryProfileInt reads leading digits",
           PrfQueryProfileInt( HINI_PROFILE, APP, "Number", 7 ), -42 );
    check( "PrfQueryProfileInt of text is 0", PrfQueryProfileInt( HINI_PROFILE, APP, "Text", 7 ), 0 );
    check( "PrfQueryProfileInt of a missing key is the default",
           PrfQueryProfileInt( HINI_PROFILE, APP, "Missing", 7 ), 7 );

    /* Sizes. */
    check( "PrfQueryProfileSize", PrfQueryProfileSize( HINI_USERPROFILE, APP, "Greeting", &size ), TRUE );
    check( "-> string size counts the NUL", size, 6 );
    check( "PrfQueryProfileSize of a missing key fails",
           PrfQueryProfileSize( HINI_USERPROFILE, APP, "Missing", &size ), FALSE );
    check_error( hab, "-> PMERR_NOT_IN_IDX", PMERR_NOT_IN_IDX );

    /* Enumeration. */
    PrfWriteProfileString( HINI_USERPROFILE, TEMP_APP, "A", "1" );
    PrfWriteProfileString( HINI_USERPROFILE, TEMP_APP, "B", "2" );
    memset( buf, 0x55, sizeof( buf ) );
    n = PrfQueryProfileString( HINI_USERPROFILE, TEMP_APP, NULL, NULL, buf, sizeof( buf ) );
    check( "NULL key enumerates the keys (length without the final NUL)", n, 4 );
    check( "-> A, B, double NUL", same( buf, "A\0B\0\0", 5 ), TRUE );
    check( "PrfQueryProfileSize of the key list",
           PrfQueryProfileSize( HINI_USERPROFILE, TEMP_APP, NULL, &size ), TRUE );
    check( "-> whole list", size, 5 );
    n = PrfQueryProfileString( HINI_USERPROFILE, NULL, NULL, NULL, buf, sizeof( buf ) );
    check( "NULL application enumerates the applications",
           same( buf, APP "\0" TEMP_APP "\0\0", sizeof( APP ) + sizeof( TEMP_APP ) + 1 ), TRUE );
    check( "-> length", n, sizeof( APP ) + sizeof( TEMP_APP ) );

    /* Binary data. */
    check( "PrfWriteProfileData",
           PrfWriteProfileData( HINI_USERPROFILE, APP, "Blob", (PVOID)blob, sizeof( blob ) ), TRUE );
    size = sizeof( buf );
    check( "PrfQueryProfileData", PrfQueryProfileData( HINI_PROFILE, APP, "Blob", buf, &size ), TRUE );
    check( "-> size", size, sizeof( blob ) );
    check( "-> bytes", same( buf, blob, sizeof( blob ) ), TRUE );
    size = 3;
    PrfQueryProfileData( HINI_PROFILE, APP, "Blob", buf, &size );
    check( "a short buffer gets what fits", size, 3 );

    /* Deleting. */
    check( "NULL string deletes a key",
           PrfWriteProfileString( HINI_USERPROFILE, TEMP_APP, "A", NULL ), TRUE );
    n = PrfQueryProfileString( HINI_USERPROFILE, TEMP_APP, NULL, NULL, buf, sizeof( buf ) );
    check( "-> one key left", n, 2 );
    check( "NULL key deletes the application",
           PrfWriteProfileData( HINI_USERPROFILE, TEMP_APP, NULL, NULL, 0 ), TRUE );
    check( "-> its keys are gone",
           PrfQueryProfileString( HINI_USERPROFILE, TEMP_APP, "B", NULL, buf, sizeof( buf ) ), 0 );
    check( "-> PrfQueryProfileSize fails",
           PrfQueryProfileSize( HINI_USERPROFILE, TEMP_APP, NULL, &size ), FALSE );

    /* A private profile. */
    hini = PrfOpenProfile( hab, PRIVATE );
    check( "PrfOpenProfile", hini != NULLHANDLE, TRUE );
    again = PrfOpenProfile( hab, "c:\\prftest.ini" );
    check( "opening it again gives the same handle", again == hini, TRUE );
    check( "write to it", PrfWriteProfileString( hini, "Private", "Key", "private value" ), TRUE );
    n = PrfQueryProfileString( hini, "Private", "Key", NULL, buf, sizeof( buf ) );
    check( "read it back", n, 14 );
    check( "the user profile does not have it",
           PrfQueryProfileString( HINI_PROFILE, "Private", "Key", NULL, buf, sizeof( buf ) ), 0 );
    check( "PrfCloseProfile", PrfCloseProfile( again ), TRUE );
    check( "PrfCloseProfile (second open)", PrfCloseProfile( hini ), TRUE );
    check( "PrfCloseProfile of a closed handle fails", PrfCloseProfile( hini ), FALSE );
    check_error( hab, "-> PMERR_INVALID_INI_FILE_HANDLE", PMERR_INVALID_INI_FILE_HANDLE );
    check( "querying a bad handle",
           PrfQueryProfileString( BAD_HINI, APP, "Greeting", "d", buf, sizeof( buf ) ), 0 );
    check_error( hab, "-> PMERR_INVALID_INI_FILE_HANDLE", PMERR_INVALID_INI_FILE_HANDLE );
    check( "PrfOpenProfile(OS2.INI) fails", PrfOpenProfile( hab, "C:\\OS2\\OS2.INI" ), NULLHANDLE );
    check_error( hab, "-> PMERR_INI_FILE_IS_SYS_OR_USER", PMERR_INI_FILE_IS_SYS_OR_USER );
}

static void help_checks( HAB hab, HWND frame, HWND client )
{
    HELPINIT  hinit;
    HWND      help;
    static USHORT subtable[] = { 2, 0 };
    static HELPTABLE table[] = { { 0 } };

    memset( &hinit, 0, sizeof( hinit ) );
    hinit.cb                  = sizeof( HELPINIT );
    hinit.ulReturnCode        = 0xDEAD;
    hinit.phtHelpTable        = (PHELPTABLE)MAKELONG( HELP_TABLE_ID, 0xFFFF );
    hinit.pszHelpWindowTitle  = "Profile Test Help";
    hinit.pszHelpLibraryName  = "PRFTEST.HLP";
    help = WinCreateHelpInstance( hab, &hinit );
    check( "WinCreateHelpInstance", help != NULLHANDLE, TRUE );
    check( "-> ulReturnCode 0", hinit.ulReturnCode, 0 );
    check( "no instance before association", WinQueryHelpInstance( client ), NULLHANDLE );
    check( "WinAssociateHelpInstance", WinAssociateHelpInstance( help, frame ), TRUE );
    check( "WinQueryHelpInstance(frame)", WinQueryHelpInstance( frame ) == help, TRUE );
    check( "WinQueryHelpInstance(client) follows the parent chain",
           WinQueryHelpInstance( client ) == help, TRUE );
    check( "WinLoadHelpTable", WinLoadHelpTable( help, HELP_TABLE_ID, NULLHANDLE ), TRUE );
    table[0].idAppWindow = 1;
    table[0].phstHelpSubTable = subtable;
    check( "WinCreateHelpTable", WinCreateHelpTable( help, table ), TRUE );
    check( "HM_DISPLAY_HELP is accepted",
           (LONG)WinSendMsg( help, HM_DISPLAY_HELP, MPFROMSHORT( 1 ), MPFROMSHORT( HM_RESOURCEID ) ), 0 );
    check( "-> no error", WinGetLastError( hab ), 0 );
    check( "HM_SET_ACTIVE_WINDOW is accepted",
           (LONG)WinSendMsg( help, HM_SET_ACTIVE_WINDOW, MPFROMHWND( frame ), MPFROMHWND( frame ) ), 0 );
    check( "disassociate", WinAssociateHelpInstance( NULLHANDLE, frame ), TRUE );
    check( "-> no instance", WinQueryHelpInstance( client ), NULLHANDLE );
    check( "WinDestroyHelpInstance", WinDestroyHelpInstance( help ), TRUE );
    check( "WinDestroyHelpInstance again fails", WinDestroyHelpInstance( help ), FALSE );
    check( "WinLoadHelpTable on a destroyed instance fails",
           WinLoadHelpTable( help, HELP_TABLE_ID, NULLHANDLE ), FALSE );
}

/* PrfQueryProfile and PrfReset. */
static void reset_checks( HAB hab )
{
    PRFPROFILE prof;
    char       user[64], sys[64];
    QMSG       qmsg;
    LONG       n;

    prof.cchUserName = 0;
    prof.cchSysName = 0;
    check( "PrfQueryProfile(lengths)", PrfQueryProfile( hab, &prof ), TRUE );
    check( "-> the user profile's name needs 15 bytes", prof.cchUserName, 15 );
    check( "-> the system profile's 18", prof.cchSysName, 18 );
    prof.pszUserName = user;
    prof.pszSysName = sys;
    check( "PrfQueryProfile(names)", PrfQueryProfile( hab, &prof ), TRUE );
    check( "-> C:\\OS2\\OS2.INI", strcmp( user, "C:\\OS2\\OS2.INI" ), 0 );
    check( "-> C:\\OS2\\OS2SYS.INI", strcmp( sys, "C:\\OS2\\OS2SYS.INI" ), 0 );
    prof.cchUserName = 5;
    prof.cchSysName = sizeof( sys );
    check( "PrfQueryProfile(short buffer) fails", PrfQueryProfile( hab, &prof ), FALSE );
    check_error( hab, "-> PMERR_BUFFER_TOO_SMALL", PMERR_BUFFER_TOO_SMALL );
    check( "-> the name cut to fit", strcmp( user, "C:\\O" ), 0 );

    prof.pszUserName = "C:\\NEWUSER.INI";
    prof.pszSysName = "C:\\OTHER.INI";
    prof.cchUserName = prof.cchSysName = 0;
    check( "PrfReset(another system profile) fails", PrfReset( hab, &prof ), FALSE );
    prof.pszSysName = "c:\\os2\\os2sys.ini";
    check( "PrfReset(C:\\NEWUSER.INI)", PrfReset( hab, &prof ), TRUE );
    check( "-> PL_ALTERED is posted",
           WinPeekMsg( hab, &qmsg, NULLHANDLE, PL_ALTERED, PL_ALTERED, PM_REMOVE ), TRUE );
    prof.pszUserName = user;
    prof.pszSysName = sys;
    prof.cchUserName = sizeof( user );
    prof.cchSysName = sizeof( sys );
    PrfQueryProfile( hab, &prof );
    check( "-> PrfQueryProfile names it", strcmp( user, "C:\\NEWUSER.INI" ), 0 );
    check( "-> the new user profile is empty",
           PrfQueryProfileString( HINI_USERPROFILE, APP, "Greeting", NULL, user, sizeof( user ) ), 0 );
    check( "-> HINI_USERPROFILE writes it",
           PrfWriteProfileString( HINI_USERPROFILE, "NewUser", "Key", "new" ), TRUE );
    n = PrfQueryProfileString( HINI_USERPROFILE, "NewUser", "Key", NULL, user, sizeof( user ) );
    check( "-> and reads it", n, 4 );
    check( "-> PrfOpenProfile of it fails", PrfOpenProfile( hab, "C:\\NEWUSER.INI" ), NULLHANDLE );
    check_error( hab, "-> PMERR_INI_FILE_IS_SYS_OR_USER", PMERR_INI_FILE_IS_SYS_OR_USER );
    prof.pszUserName = "C:\\OS2\\OS2.INI";
    prof.pszSysName = "C:\\OS2\\OS2SYS.INI";
    check( "PrfReset(back to OS2.INI)", PrfReset( hab, &prof ), TRUE );
    WinPeekMsg( hab, &qmsg, NULLHANDLE, PL_ALTERED, PL_ALTERED, PM_REMOVE );
    n = PrfQueryProfileString( HINI_USERPROFILE, APP, "Greeting", NULL, user, sizeof( user ) );
    check( "-> its data is back", n, 6 );
}

int main( void )
{
    HAB    hab;
    HMQ    hmq;
    HWND   frame, client;
    ULONG  flags = FCF_TITLEBAR | FCF_SYSMENU;
    char   line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "ProfileTest", ClientWndProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "ProfileTest",
                                "PM Profile Test", 0, NULLHANDLE, 0, &client );

    profile_checks( hab );
    reset_checks( hab );
    help_checks( hab, frame, client );

    WinDestroyWindow( frame );
    p = append_str( line, "pm_profile_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
