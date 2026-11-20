/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_stddlg_test.c - the standard file and font dialogs (PMCTLS) and the
 * printing calls (PMSPL, DevOpenDC / DevEscape / DevPostDeviceModes)
 * under Warpine, which has no printer.
 *
 * main() first creates C:\STDDLG with ALPHA.TXT, BETA.TXT and NOTES.DOC,
 * checks the structure sizes against <pmstddlg.h> and the spooler and
 * device calls (empty lists, documented errors). Then it opens:
 *
 *   1. WinFileDlg: an Open dialog on "C:\STDDLG\*.TXT", centred, subclassed
 *      by FileProc (which counts the files it lists at WM_INITDLG and
 *      checks the FILEDLG in QWL_USER, passing everything on to
 *      WinDefFileDlgProc). The integration test types "beta.txt" and
 *      Enter; the dialog returns "C:\STDDLG\BETA.TXT" (the case on disk).
 *   2. WinFontDlg on "IBM Plex Sans" 12 pt; the test presses Down (the
 *      next family, IBM Plex Serif) and Enter.
 *
 * With no session (WARPINE_HEADLESS, no script) the dialogs wait. Each
 * check is written to stdout:
 *
 *   pm_stddlg_test: <name>: ok
 *   pm_stddlg_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_stddlg_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DEV
#define INCL_SPL
#define INCL_SPLDOSPRINT
#define INCL_SPLERRORS
#define INCL_WINSTDFILE
#define INCL_WINSTDFONT
#define INCL_DOSFILEMGR
#define INCL_DOSPROCESS
#define INCL_DOSERRORS
#define INCL_ERRORS
#define INCL_GPIERRORS
#include <os2.h>
#include <string.h>

static HAB  hab;
static HWND frame, client;
static int  n_checks = 0;
static int  n_passed = 0;

/* What FileProc saw. */
static LONG files_listed = -1;
static LONG user_word_ok = 0;
static LONG proc_calls = 0;

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
    p = append_str( line, "pm_stddlg_test: " );
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
        p = append_str( line, "pm_stddlg_test: (" );
        p = append_str( p, what );
        p = append_str( p, " is \"" );
        p = append_str( p, got );
        append_str( p, "\")\r\n" );
        write_stdout( line );
    }
    check( what, strcmp( got, want ) == 0, TRUE );
}

/* ---------- files to browse ---------- */

static void make_file( PSZ name )
{
    HFILE  h;
    ULONG  action, written;
    if( DosOpen( name, &h, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                 OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYNONE, NULL ) == NO_ERROR ) {
        DosWrite( h, "x", 1, &written );
        DosClose( h );
    }
}

/* ---------- printing ---------- */

static void check_printing( void )
{
    BYTE         buf[256];
    ULONG        ret = 99, total = 99, needed = 99;
    PSZ          dop[4];
    HSPL         hspl;
    HDC          hdc, mem;
    DEVOPENSTRUC dos;
    LONG         out = 0;
    SPLERR       rc;

    rc = SplEnumQueue( NULL, 3, buf, sizeof( buf ), &ret, &total, &needed, NULL );
    check( "SplEnumQueue", rc, NO_ERROR );
    check( "SplEnumQueue no queues", ret + total + needed, 0 );
    ret = 99;
    rc = SplEnumPrinter( NULL, 0, SPL_PR_QUEUE | SPL_PR_DIRECT_DEVICE, buf, sizeof( buf ),
                         &ret, &total, &needed, NULL );
    check( "SplEnumPrinter", rc, NO_ERROR );
    check( "SplEnumPrinter no printers", ret, 0 );
    check( "SplEnumPrinter bad level", SplEnumPrinter( NULL, 1, SPL_PR_QUEUE, buf, sizeof( buf ),
                                                       &ret, &total, &needed, NULL ), ERROR_INVALID_LEVEL );
    ret = 99;
    check( "SplEnumDevice", SplEnumDevice( NULL, 0, buf, sizeof( buf ), &ret, &total, &needed, NULL ), NO_ERROR );
    check( "SplEnumDevice no devices", ret, 0 );
    check( "SplQueryQueue", SplQueryQueue( NULL, "LPT1Q", 3, buf, sizeof( buf ), &needed ), NERR_QNotFound );

    dop[0] = "LPT1Q"; dop[1] = "IBMNULL"; dop[2] = NULL; dop[3] = "PM_Q_STD";
    hspl = SplQmOpen( "*", 4, dop );
    check( "SplQmOpen fails", hspl, NULLHANDLE );
    check( "SplQmOpen error", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_SPL_QUEUE_NOT_FOUND );
    check( "SplQmClose bad handle", SplQmClose( 1 ), FALSE );
    check( "SplQmClose error", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_SPL_INV_HSPL );

    memset( &dos, 0, sizeof( dos ) );
    dos.pszLogAddress = "LPT1Q";
    dos.pszDriverName = "IBMNULL";
    hdc = DevOpenDC( hab, OD_QUEUED, "*", 2, (PDEVOPENDATA)&dos, NULLHANDLE );
    check( "DevOpenDC OD_QUEUED", hdc, DEV_ERROR );
    check( "DevOpenDC error", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_DRIVER_NOT_FOUND );
    check( "DevOpenDC OD_DIRECT", DevOpenDC( hab, OD_DIRECT, "*", 2, (PDEVOPENDATA)&dos, NULLHANDLE ), DEV_ERROR );

    mem = DevOpenDC( hab, OD_MEMORY, "*", 0, NULL, NULLHANDLE );
    check( "DevOpenDC OD_MEMORY", mem != DEV_ERROR, TRUE );
    check( "DevEscape STARTDOC", DevEscape( mem, DEVESC_STARTDOC, 4, "Doc", &out, NULL ), 0 /* DEVESC_NOTIMPLEMENTED */ );
    check( "DevEscape bad hdc", DevEscape( (HDC)0xBAD, DEVESC_QUERYESCSUPPORT, 0, NULL, &out, NULL ), -1 /* DEVESC_ERROR */ );
    check( "DevEscape error", ERRORIDERROR( WinGetLastError( hab ) ), PMERR_INV_HDC );
    DevCloseDC( mem );
    check( "DevPostDeviceModes", DevPostDeviceModes( hab, NULL, "IBMNULL", "", NULL, DPDM_POSTJOBPROP ), DPDM_ERROR );
}

/* ---------- the file dialog ---------- */

static MRESULT EXPENTRY FileProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    proc_calls++;
    if( msg == WM_INITDLG ) {
        PFILEDLG pfd = (PFILEDLG)WinQueryWindowULong( hwnd, QWL_USER );
        user_word_ok = pfd != NULL && pfd->fl == ( FDS_OPEN_DIALOG | FDS_CENTER ) && (PVOID)pfd == PVOIDFROMMP( mp2 );
        files_listed = (LONG)WinSendDlgItemMsg( hwnd, DID_FILES_LB, LM_QUERYITEMCOUNT, 0, 0 );
    }
    return WinDefFileDlgProc( hwnd, msg, mp1, mp2 );
}

static void check_file_dialog( void )
{
    FILEDLG fd;
    HWND    rc;

    memset( &fd, 0, sizeof( fd ) );
    fd.cbSize = sizeof( fd );
    fd.fl = FDS_OPEN_DIALOG | FDS_CENTER;
    fd.pszTitle = "Open a text file";
    fd.pfnDlgProc = FileProc;
    strcpy( fd.szFullFile, "C:\\STDDLG\\*.TXT" );
    rc = WinFileDlg( HWND_DESKTOP, frame, &fd );
    check( "WinFileDlg returns TRUE", (LONG)rc, TRUE );
    check( "FILEDLG lReturn", fd.lReturn, DID_OK );
    check( "FILEDLG lSRC", fd.lSRC, FDS_SUCCESSFUL );
    check_str( "FILEDLG szFullFile", fd.szFullFile, "C:\\STDDLG\\BETA.TXT" );
    check( "FILEDLG ulFQFCount", fd.ulFQFCount, 1 );
    check( "dialog procedure saw the *.TXT files", files_listed, 2 );
    check( "QWL_USER and WM_INITDLG mp2 are the FILEDLG", user_word_ok, TRUE );
    check( "dialog procedure called", proc_calls > 3, TRUE );
    check( "WinFreeFileDlgList foreign list", WinFreeFileDlgList( (PAPSZ)&fd ), FALSE );
}

/* ---------- the font dialog ---------- */

static void check_font_dialog( void )
{
    FONTDLG fd;
    char    family[FACESIZE];
    HWND    rc;
    HPS     hps;

    memset( &fd, 0, sizeof( fd ) );
    strcpy( family, "IBM Plex Sans" );
    fd.cbSize = sizeof( fd );
    fd.fl = FNTS_CENTER | FNTS_RESETBUTTON;
    fd.pszFamilyname = family;
    fd.usFamilyBufLen = sizeof( family );
    fd.fxPointSize = MAKEFIXED( 12, 0 );
    fd.clrFore = CLR_BLACK;
    fd.clrBack = CLR_WHITE;
    fd.pszPreview = "Warpine";
    fd.fAttrs.usCodePage = 850;
    rc = WinFontDlg( HWND_DESKTOP, frame, &fd );
    check( "WinFontDlg returns TRUE", (LONG)rc, TRUE );
    check( "FONTDLG lReturn", fd.lReturn, DID_OK );
    check_str( "FONTDLG pszFamilyname", family, "IBM Plex Serif" );
    check( "FONTDLG fxPointSize", fd.fxPointSize, MAKEFIXED( 12, 0 ) );
    check( "FONTDLG sNominalPointSize", fd.sNominalPointSize, 120 );
    check( "FONTDLG flStyle", fd.flStyle, 0 );
    check( "FONTDLG usWeight", fd.usWeight, FWEIGHT_NORMAL );
    check( "FATTRS usRecordLength", fd.fAttrs.usRecordLength, sizeof( FATTRS ) );
    check_str( "FATTRS szFacename", fd.fAttrs.szFacename, "IBM Plex Serif" );
    check( "FATTRS usCodePage kept", fd.fAttrs.usCodePage, 850 );
    check( "FATTRS lMaxBaselineExt", fd.fAttrs.lMaxBaselineExt > 0, TRUE );
    hps = WinGetPS( client );
    check( "GpiCreateLogFont from the dialog", GpiCreateLogFont( hps, NULL, 1, &fd.fAttrs ) != GPI_ERROR, TRUE );
    WinReleasePS( hps );
}

/* ---------- the client ---------- */

static MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    if( msg == WM_PAINT ) {
        RECTL rc;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, &rc );
        WinFillRect( hps, &rc, CLR_PALEGRAY );
        WinEndPaint( hps );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    ULONG flags = FCF_TITLEBAR | FCF_SYSMENU | FCF_BORDER;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "StdDlgTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flags, "StdDlgTest",
                                "PM Standard Dialog Test", WS_VISIBLE, NULLHANDLE, 0, &client );

    DosCreateDir( "C:\\STDDLG", NULL );
    make_file( "C:\\STDDLG\\ALPHA.TXT" );
    make_file( "C:\\STDDLG\\BETA.TXT" );
    make_file( "C:\\STDDLG\\NOTES.DOC" );

    check( "sizeof(FILEDLG)", sizeof( FILEDLG ), 328 );
    check( "sizeof(FONTDLG)", sizeof( FONTDLG ), 168 );
    check_printing();
    check_file_dialog();
    check_font_dialog();

    p = append_str( line, "pm_stddlg_test: done " );
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
