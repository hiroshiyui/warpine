/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_modal_filter_test.c - message-filter hooks in the modal loops and the
 * messages the standard dialogs send their dialog procedure, under Warpine
 *
 * The program drives itself (no input needed):
 *
 *   1. An HK_MSGFILTER hook on the queue. The client posts itself three
 *      messages and opens WinMessageBox: its modal loop passes each to the
 *      hook with MSGF_MESSAGEBOX. The hook swallows the first, changes mp1
 *      of the third, and lets the second through, whose handler closes the
 *      box with WM_COMMAND(MBID_OK). WinGetMsg in the program's own loop
 *      does not call the hook.
 *   2. WinFileDlg on C:\FILTER\* with a dialog procedure, which sees
 *      MSGF_DIALOGBOX in the hook and:
 *        FDM_FILTER   refuses the *.SKIP file (two of three files listed);
 *        FDM_ERROR    for "Q:\NONE.TXT" (no drive Q:);
 *        FDM_VALIDATE refuses BAD.TXT, then accepts KEEP1.TXT (the result).
 *   3. WinFontDlg with a dialog procedure: FNTM_FILTERLIST refuses
 *      "IBM Plex Mono" (two families listed); choosing the second family
 *      sends FNTM_FACENAMECHANGED, after which the procedure ends the
 *      dialog with OK.
 *
 * Each check is written to stdout:
 *
 *   pm_modal_filter_test: <name>: ok
 *   pm_modal_filter_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_modal_filter_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_WINSTDFILE
#define INCL_WINSTDFONT
#define INCL_DOSFILEMGR
#define INCL_DOSPROCESS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

/* Not in the Open Watcom 32-bit <pmwin.h>; PM Reference / 1.x header. */
#ifndef MSGF_DIALOGBOX
#define MSGF_DIALOGBOX  1
#define MSGF_MESSAGEBOX 2
#endif

#define MSG_SWALLOWED ( WM_USER + 1 )
#define MSG_CLOSE_BOX ( WM_USER + 2 )
#define MSG_CHANGED   ( WM_USER + 3 )
#define MSG_OUTSIDE   ( WM_USER + 4 )

static HAB  hab;
static HWND frame, client;
static int  n_checks = 0;
static int  n_passed = 0;

/* What the hook saw. */
static int   hook_calls = 0;
static int   hook_saw_swallowed = 0;
static ULONG hook_msgf_box = 0;
static int   hook_dialogbox = 0;
static int   hook_saw_outside = 0;
static int   msgbox_running = 0;

/* What the client got. */
static int   got_swallowed = 0;
static int   got_close = 0;
static ULONG got_changed_mp1 = 0;

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
    p = append_str( line, "pm_modal_filter_test: " );
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

static void check_str( const char *what, const char *got, const char *want )
{
    if( strcmp( got, want ) != 0 ) {
        char line[400], *p;
        p = append_str( line, "pm_modal_filter_test: (" );
        p = append_str( p, what );
        p = append_str( p, " is \"" );
        p = append_str( p, got );
        append_str( p, "\")\r\n" );
        write_stdout( line );
    }
    check( what, strcmp( got, want ) == 0, TRUE );
}

static int ends_with( const char *s, const char *tail )
{
    size_t n = strlen( s ), m = strlen( tail );
    return n >= m && strcmp( s + n - m, tail ) == 0;
}

/* ---------- the hook ---------- */

static BOOL EXPENTRY FilterHook( HAB h, PQMSG pq, ULONG msgf )
{
    (void)h;
    hook_calls++;
    if( msgf == MSGF_DIALOGBOX )
        hook_dialogbox++;
    switch( pq->msg ) {
    case MSG_SWALLOWED:
        hook_saw_swallowed++;
        hook_msgf_box = msgf;
        return TRUE;
    case MSG_CHANGED:
        pq->mp1 = MPFROMLONG( 77 );
        return FALSE;
    case MSG_OUTSIDE:
        hook_saw_outside++;
        return FALSE;
    }
    return FALSE;
}

/* ---------- the client ---------- */

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case MSG_SWALLOWED:
        got_swallowed++;
        return 0;
    case MSG_CHANGED:
        got_changed_mp1 = LONGFROMMP( mp1 );
        return 0;
    case MSG_CLOSE_BOX: {
        HWND box = WinQueryActiveWindow( HWND_DESKTOP );
        got_close++;
        if( msgbox_running && box != frame )
            WinPostMsg( box, WM_COMMAND, MPFROMSHORT( MBID_OK ), MPFROM2SHORT( CMDSRC_PUSHBUTTON, FALSE ) );
        return 0;
    }
    case MSG_OUTSIDE:
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

/* ---------- 1. the message box ---------- */

static void check_message_box( void )
{
    ULONG rc;
    QMSG  qmsg;

    check( "WinSetHook(HK_MSGFILTER)",
           WinSetHook( hab, HMQ_CURRENT, HK_MSGFILTER, (PFN)FilterHook, NULLHANDLE ), TRUE );
    WinPostMsg( client, MSG_SWALLOWED, 0, 0 );
    WinPostMsg( client, MSG_CLOSE_BOX, 0, 0 );
    WinPostMsg( client, MSG_CHANGED, MPFROMLONG( 5 ), 0 );
    msgbox_running = 1;
    rc = WinMessageBox( HWND_DESKTOP, frame, "Closed by the program", "Filter", 0, MB_OK );
    msgbox_running = 0;
    check( "WinMessageBox closed with MBID_OK", rc, MBID_OK );
    check( "the hook saw the swallowed message", hook_saw_swallowed, 1 );
    check( "  with MSGF_MESSAGEBOX", hook_msgf_box, MSGF_MESSAGEBOX );
    check( "the swallowed message did not reach the client", got_swallowed, 0 );
    check( "the message let through did", got_close, 1 );
    check( "the hook changed mp1 of the third", got_changed_mp1, 77 );

    /* The program's own WinGetMsg loop does not call the filter hook. */
    WinPostMsg( client, MSG_OUTSIDE, 0, 0 );
    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );
    check( "WinGetMsg does not call the filter hook", hook_saw_outside, 0 );
}

/* ---------- 2. the file dialog ---------- */

static int  filter_calls = 0, filter_refused = 0, files_listed = -1;
static int  validate_calls = 0, error_calls = 0;
static LONG error_code = -1;
static char validated_first[CCHMAXPATH] = "";
static LONG validate_field = -1;
static int  file_step = 0;

static void make_file( PSZ name )
{
    HFILE h;
    ULONG action, written;
    if( DosOpen( name, &h, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                 OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYNONE, NULL ) == NO_ERROR ) {
        DosWrite( h, "x", 1, &written );
        DosClose( h );
    }
}

static void type_and_ok( HWND hwnd, PSZ text )
{
    WinSetDlgItemText( hwnd, DID_FILENAME_ED, text );
    WinPostMsg( hwnd, WM_COMMAND, MPFROMSHORT( DID_OK ), MPFROM2SHORT( CMDSRC_PUSHBUTTON, FALSE ) );
}

static MRESULT EXPENTRY FileProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    MRESULT r;
    switch( msg ) {
    case WM_INITDLG:
        /* The default procedure fills the lists through FDM_FILTER. */
        r = WinDefFileDlgProc( hwnd, msg, mp1, mp2 );
        files_listed = (LONG)WinSendDlgItemMsg( hwnd, DID_FILES_LB, LM_QUERYITEMCOUNT, 0, 0 );
        type_and_ok( hwnd, "Q:\\NONE.TXT" );
        return r;
    case FDM_FILTER:
        filter_calls++;
        if( ends_with( (char *)PVOIDFROMMP( mp1 ), ".SKIP" ) ) {
            filter_refused++;
            return (MRESULT)FALSE;
        }
        return (MRESULT)TRUE;
    case FDM_ERROR:
        error_calls++;
        error_code = SHORT1FROMMP( mp1 );
        if( file_step == 0 ) {
            file_step = 1;
            type_and_ok( hwnd, "BAD.TXT" );
        }
        return (MRESULT)MBID_OK;
    case FDM_VALIDATE:
        validate_calls++;
        if( file_step == 1 ) {
            file_step = 2;
            strcpy( validated_first, (char *)PVOIDFROMMP( mp1 ) );
            validate_field = SHORT1FROMMP( mp2 );
            type_and_ok( hwnd, "KEEP1.TXT" );
            return (MRESULT)FALSE;
        }
        return (MRESULT)TRUE;
    }
    return WinDefFileDlgProc( hwnd, msg, mp1, mp2 );
}

static void check_file_dialog( void )
{
    FILEDLG fd;
    HWND    rc;

    DosCreateDir( "C:\\FILTER", NULL );
    make_file( "C:\\FILTER\\KEEP1.TXT" );
    make_file( "C:\\FILTER\\DROP.SKIP" );
    make_file( "C:\\FILTER\\KEEP2.TXT" );
    hook_dialogbox = 0;
    memset( &fd, 0, sizeof( fd ) );
    fd.cbSize = sizeof( fd );
    fd.fl = FDS_OPEN_DIALOG | FDS_CENTER;
    fd.pfnDlgProc = FileProc;
    strcpy( fd.szFullFile, "C:\\FILTER\\*" );
    rc = WinFileDlg( HWND_DESKTOP, frame, &fd );
    check( "WinFileDlg returns TRUE", (LONG)rc, TRUE );
    check( "FILEDLG lReturn", fd.lReturn, DID_OK );
    check_str( "FILEDLG szFullFile", fd.szFullFile, "C:\\FILTER\\KEEP1.TXT" );
    check( "FDM_FILTER for each file", filter_calls, 3 );
    check( "FDM_FILTER refused one", filter_refused, 1 );
    check( "the refused file is not listed", files_listed, 2 );
    check( "FDM_ERROR for a missing drive", error_calls, 1 );
    check( "  FDS_ERR_INVALID_DRIVE", error_code, 4 /* FDS_ERR_INVALID_DRIVE */ );
    check( "FDM_VALIDATE twice", validate_calls, 2 );
    check_str( "  with the fully qualified name", validated_first, "C:\\FILTER\\BAD.TXT" );
    check( "  from the entry field", validate_field, DID_FILENAME_ED );
    check( "the hook saw the dialog's messages (MSGF_DIALOGBOX)", hook_dialogbox > 0, TRUE );
}

/* ---------- 3. the font dialog ---------- */

static int  list_calls = 0, list_refused = 0, names_listed = -1, face_changes = 0;
static char changed_face[FACESIZE] = "";

static MRESULT EXPENTRY FontProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    MRESULT r;
    switch( msg ) {
    case WM_INITDLG:
        r = WinDefFontDlgProc( hwnd, msg, mp1, mp2 );
        names_listed = (LONG)WinSendDlgItemMsg( hwnd, DID_NAME, LM_QUERYITEMCOUNT, 0, 0 );
        /* Choose the second family left; the default procedure tells us. */
        WinSendDlgItemMsg( hwnd, DID_NAME, LM_SELECTITEM, MPFROMSHORT( 1 ), MPFROMSHORT( TRUE ) );
        WinDefFontDlgProc( hwnd, WM_CONTROL, MPFROM2SHORT( DID_NAME, CBN_LBSELECT ), 0 );
        return r;
    case FNTM_FILTERLIST:
        list_calls++;
        if( SHORT1FROMMP( mp2 ) == DID_NAME && strcmp( (char *)PVOIDFROMMP( mp1 ), "IBM Plex Mono" ) == 0 ) {
            list_refused++;
            return (MRESULT)FALSE;
        }
        return (MRESULT)TRUE;
    case FNTM_FACENAMECHANGED:
        face_changes++;
        strcpy( changed_face, (char *)PVOIDFROMMP( mp1 ) );
        WinPostMsg( hwnd, WM_COMMAND, MPFROMSHORT( DID_OK ), MPFROM2SHORT( CMDSRC_PUSHBUTTON, FALSE ) );
        return 0;
    }
    return WinDefFontDlgProc( hwnd, msg, mp1, mp2 );
}

static void check_font_dialog( void )
{
    FONTDLG fd;
    char    family[FACESIZE];
    HWND    rc;

    memset( &fd, 0, sizeof( fd ) );
    strcpy( family, "IBM Plex Sans" );
    fd.cbSize = sizeof( fd );
    fd.fl = FNTS_CENTER;
    fd.pfnDlgProc = FontProc;
    fd.pszFamilyname = family;
    fd.usFamilyBufLen = sizeof( family );
    fd.fxPointSize = MAKEFIXED( 12, 0 );
    fd.clrFore = CLR_BLACK;
    fd.clrBack = CLR_WHITE;
    rc = WinFontDlg( HWND_DESKTOP, frame, &fd );
    check( "WinFontDlg returns TRUE", (LONG)rc, TRUE );
    check( "FONTDLG lReturn", fd.lReturn, DID_OK );
    check( "FNTM_FILTERLIST for families and sizes", list_calls > 3, TRUE );
    check( "FNTM_FILTERLIST refused one family", list_refused, 1 );
    check( "two families listed", names_listed, 2 );
    check( "FNTM_FACENAMECHANGED once", face_changes, 1 );
    check_str( "  with the new family", changed_face, "IBM Plex Serif" );
    check_str( "FONTDLG pszFamilyname", family, "IBM Plex Serif" );
}

int main( void )
{
    HMQ   hmq;
    ULONG fl = FCF_TITLEBAR | FCF_SYSMENU | FCF_SIZEBORDER;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "ModalFilter", ClientWndProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &fl, "ModalFilter", "Modal filters",
                                0, NULLHANDLE, 0, &client );

    check_message_box();
    check_file_dialog();
    check_font_dialog();
    check( "WinReleaseHook", WinReleaseHook( hab, HMQ_CURRENT, HK_MSGFILTER, (PFN)FilterHook, NULLHANDLE ), TRUE );

    p = append_str( line, "pm_modal_filter_test: done " );
    p = append_long( p, n_passed );
    p = append_str( p, "/" );
    p = append_long( p, n_checks );
    append_str( p, "\r\n" );
    write_stdout( line );

    WinDestroyWindow( frame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return n_passed == n_checks ? 0 : 1;
}
