/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_winutil_test.c - PMWIN utility calls (ADR-0086): string helpers in a
 * codepage, the message clock, dialog units, a dialog from a template in
 * memory and its own message loop, presentation spaces, inverting, window
 * visibility and the clipboard's owner, viewer and formats.
 *
 * On its first WM_PAINT the window runs its checks and writes each one to
 * stdout:
 *
 *   pm_winutil_test: <name>: ok
 *   pm_winutil_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_winutil_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOS
#define INCL_WINSEI
#include <os2.h>
#include <string.h>

#define PREFIX     "pm_winutil_test: "
#define ID_ENTRY   101
#define FMT_TEST   0x4321
#define WM_PING    (WM_USER + 7)

/* Calls the Open Watcom headers leave out (Toolkit pmwin.h). */
/* ERRINFO as the Warp 4.5 Toolkit declares it (ULONG fields; Open Watcom's
   pmwin.h has the older USHORT ones). */
typedef struct { ULONG cbFixedErrInfo; ERRORID idError; ULONG cDetailLevel; ULONG offaoffszMsg; ULONG offBinaryData; } ERRINFO32;
BOOL  APIENTRY WinGetDlgMsg(HWND hwndDlg, PQMSG pqmsg);
ULONG APIENTRY WinEnumClipbrdFmts(HAB hab, ULONG fmt);

static HAB  hab;
static HWND hwndFrame;
static BOOL checked = FALSE;
static int  n_checks = 0, n_passed = 0;

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

static void check( const char *name, LONG got, LONG want )
{
    char line[160];
    char *p = append_str( line, PREFIX );
    p = append_str( p, name );
    if( got == want ) {
        p = append_str( p, ": ok" );
        n_passed++;
    } else {
        p = append_str( p, ": FAIL got " );
        p = append_long( p, got );
        p = append_str( p, " want " );
        p = append_long( p, want );
    }
    append_str( p, "\r\n" );
    write_stdout( line );
    n_checks++;
}

/* ---------- strings ---------- */

static void string_checks( void )
{
    char   buf[32], out[16];
    PCSZ   s = "ab";
    ULONG  t1, t2;

    strcpy( buf, "hello \x82!" );                       /* e-acute in codepage 850 */
    check( "WinUpper length", WinUpper( hab, 850, 0, buf ), 8 );
    check( "WinUpper text", strcmp( buf, "HELLO \x90!" ), 0 );
    check( "WinUpperChar", WinUpperChar( hab, 850, 0, 'q' ), 'Q' );
    check( "WinNextChar", WinNextChar( hab, 0, 0, s ) - s, 1 );
    check( "WinNextChar at the end", WinNextChar( hab, 0, 0, s + 2 ) - s, 2 );
    check( "WinPrevChar", WinPrevChar( hab, 0, 0, s, s + 2 ) - s, 1 );
    check( "WinPrevChar at the start", WinPrevChar( hab, 0, 0, s, s ) - s, 0 );
    check( "WinCompareStrings less", WinCompareStrings( hab, 0, 0, "apple", "Banana", 0 ), WCS_LT );
    check( "WinCompareStrings equal", WinCompareStrings( hab, 0, 0, "same", "same", 0 ), WCS_EQ );
    check( "WinCompareStrings greater", WinCompareStrings( hab, 0, 0, "b", "a", 0 ), WCS_GT );
    check( "WinCpTranslateChar", WinCpTranslateChar( hab, 850, 0x82, 1252 ), 0xE9 );
    check( "WinCpTranslateString", WinCpTranslateString( hab, 850, "\x82t\x82", 1252, sizeof out, out ), TRUE );
    check( "WinCpTranslateString text", strcmp( out, "\xE9t\xE9" ), 0 );
    check( "WinCpTranslateString cut", WinCpTranslateString( hab, 850, "abcdef", 850, 3, out ) && strcmp( out, "ab" ) == 0, TRUE );
    t1 = WinGetCurrentTime( hab );
    DosSleep( 60 );
    t2 = WinGetCurrentTime( hab );
    check( "WinGetCurrentTime advances", t2 - t1 >= 40 && t2 - t1 < 5000, TRUE );
}

/* ---------- dialogs ---------- */

static BYTE tmpl[160];

static void put16( int off, USHORT v ) { tmpl[off] = (BYTE)v; tmpl[off + 1] = (BYTE)( v >> 8 ); }
static void put32( int off, ULONG v ) { put16( off, (USHORT)v ); put16( off + 2, (USHORT)( v >> 16 ) ); }

/* One DLGTITEM at off: predefined class `atom`, text at text_off. */
static void item( int off, USHORT children, USHORT atom, USHORT cch, USHORT text_off, ULONG style,
                  SHORT x, SHORT y, SHORT cx, SHORT cy, USHORT id )
{
    put16( off, 0 ); put16( off + 2, children ); put16( off + 4, 0 ); put16( off + 6, atom );
    put16( off + 8, cch ); put16( off + 10, text_off ); put32( off + 12, style );
    put16( off + 16, x ); put16( off + 18, y ); put16( off + 20, cx ); put16( off + 22, cy );
    put16( off + 24, id ); put16( off + 26, 0xFFFF ); put16( off + 28, 0xFFFF );
}

static void dialog_checks( HWND hwndClient )
{
    HWND   dlg;
    QMSG   q;
    POINTL pt;
    int    i;
    BOOL   got_ping = FALSE;

    /* Header, the dialog (a frame) and an entry field holding "abc". */
    memset( tmpl, 0, sizeof tmpl );
    put16( 0, 84 ); put16( 2, 0 ); put16( 4, 850 ); put16( 6, 14 ); put16( 8, 0 ); put16( 10, 0 ); put16( 12, 0 );
    item( 14, 1, 1, 4, 74, FS_DLGBORDER, 10, 10, 120, 60, 100 );
    item( 44, 0, 6, 4, 78, WS_VISIBLE | ES_MARGIN, 5, 5, 60, 12, ID_ENTRY );
    memcpy( tmpl + 74, "Dlg", 4 );
    memcpy( tmpl + 78, "abc", 4 );
    dlg = WinCreateDlg( HWND_DESKTOP, hwndClient, WinDefDlgProc, (PDLGTEMPLATE)tmpl, NULL );
    check( "WinCreateDlg", dlg != NULLHANDLE, TRUE );
    check( "WinCreateDlg item", WinWindowFromID( dlg, ID_ENTRY ) != NULLHANDLE, TRUE );
    check( "WinQueryDlgItemTextLength", WinQueryDlgItemTextLength( dlg, ID_ENTRY ), 3 );

    pt.x = 8; pt.y = 16;
    check( "WinMapDlgPoints to pels", WinMapDlgPoints( dlg, &pt, 1, TRUE ), TRUE );
    check( "WinMapDlgPoints gives pels", pt.x > 8 && pt.y > 16, TRUE );
    WinMapDlgPoints( dlg, &pt, 1, FALSE );
    check( "WinMapDlgPoints back", pt.x == 8 && pt.y == 16, TRUE );

    /* The dialog's own loop: messages until it is dismissed. */
    WinPostMsg( dlg, WM_PING, 0, 0 );
    for( i = 0; i < 20 && !got_ping; i++ ) {
        if( !WinGetDlgMsg( dlg, &q ) ) break;
        if( q.msg == WM_PING ) got_ping = TRUE;
        else WinDispatchMsg( hab, &q );
    }
    check( "WinGetDlgMsg", got_ping, TRUE );
    WinDismissDlg( dlg, 1 );
    check( "WinGetDlgMsg after dismissal", WinGetDlgMsg( dlg, &q ), FALSE );
    WinDestroyWindow( dlg );
}

/* ---------- painting and windows ---------- */

static LONG pel_rgb( HPS hps, LONG x, LONG y )
{
    POINTL pt;
    pt.x = x; pt.y = y;
    return GpiQueryPel( hps, &pt );
}

static void paint_checks( HWND hwndClient )
{
    HPS   hps;
    RECTL r;
    HWND  hidden;

    hps = WinGetClipPS( hwndClient, NULLHANDLE, PSF_CLIPSIBLINGS );
    check( "WinGetClipPS", hps != NULLHANDLE, TRUE );
    GpiCreateLogColorTable( hps, 0, LCOLF_RGB, 0, 0, NULL );
    WinSetRect( hab, &r, 0, 0, 40, 40 );
    WinFillRect( hps, &r, 0xFFFFFF );
    WinSetRect( hab, &r, 10, 10, 20, 20 );
    check( "WinInvertRect", WinInvertRect( hps, &r ), TRUE );
    check( "WinInvertRect inverts", pel_rgb( hps, 15, 15 ), 0 );
    check( "WinInvertRect leaves the rest", pel_rgb( hps, 25, 25 ), 0xFFFFFF );
    WinInvertRect( hps, &r );
    check( "WinInvertRect twice restores", pel_rgb( hps, 15, 15 ), 0xFFFFFF );
    WinFillRect( hps, &r, 0 );
    check( "WinFillRect RGB black", pel_rgb( hps, 15, 15 ), 0 );
    check( "WinGetClipPS released", WinReleasePS( hps ), TRUE );
    check( "WinGetClipPS bad flag", WinGetClipPS( hwndClient, NULLHANDLE, 0x8000 ), NULLHANDLE );

    hps = WinGetScreenPS( HWND_DESKTOP );
    check( "WinGetScreenPS", hps != NULLHANDLE, TRUE );
    check( "WinGetScreenPS released", WinReleasePS( hps ), TRUE );

    check( "WinMakeRect", WinMakeRect( hab, &r ), TRUE );
    check( "WinMakePoints", WinMakePoints( hab, (PPOINTL)&r, 2 ), TRUE );

    check( "WinIsWindowShowing", WinIsWindowShowing( hwndClient ), TRUE );
    hidden = WinCreateWindow( hwndClient, WC_STATIC, "", 0, 0, 0, 10, 10, hwndClient, HWND_TOP, 7, NULL, NULL );
    check( "WinIsWindowShowing hidden", WinIsWindowShowing( hidden ), FALSE );
    WinShowWindow( hidden, TRUE );
    check( "WinIsWindowShowing shown", WinIsWindowShowing( hidden ), TRUE );
    WinSetWindowPos( hidden, HWND_TOP, -50, -50, 10, 10, SWP_MOVE );
    check( "WinIsWindowShowing outside its parent", WinIsWindowShowing( hidden ), FALSE );
    WinDestroyWindow( hidden );
}

/* ---------- waits, pointers, errors, libraries, positions ---------- */

static void wait_checks( void )
{
    HEV        hev;
    HMUX       hmux;
    SEMRECORD  rec;
    ULONG      user = 0;
    LONG       input = WinCheckInput( hab );

    DosCreateEventSem( NULL, &hev, 0, TRUE );
    rec.hsemCur = (HSEM)hev;
    rec.ulUser = 77;
    DosCreateMuxWaitSem( NULL, &hmux, 1, &rec, DCMW_WAIT_ANY );
    check( "WinWaitMuxWaitSem", WinWaitMuxWaitSem( hmux, 0, &user ), 0 );
    check( "WinWaitMuxWaitSem user", user, 77 );
    DosCloseMuxWaitSem( hmux );
    DosCloseEventSem( hev );
    check( "WinCheckInput", input == TRUE || input == FALSE, TRUE );
    check( "WinIsThreadActive", WinIsThreadActive( hab ), TRUE );
}

static void fill( HPS hps, LONG l, LONG b, LONG r, LONG t, LONG rgb )
{
    RECTL rc;
    WinSetRect( hab, &rc, l, b, r, t );
    WinFillRect( hps, &rc, rgb );
}

static void pointer_checks( HWND hwndClient )
{
    SIZEL            size = { 16, 32 };
    BITMAPINFOHEADER2 bmih;
    POINTERINFO      pi;
    HDC              hdc;
    HPS              mps, hps;
    HBITMAP          hbm;
    HPOINTER         hptr;

    /* The masks, 16 x 32: AND above (pels 0-7 of each row set), XOR below
       (pels 0-3 set). So pels 0-3 invert, 4-7 are transparent, 8-15 black. */
    hdc = DevOpenDC( hab, OD_MEMORY, "*", 0, NULL, NULLHANDLE );
    mps = GpiCreatePS( hab, hdc, &size, PU_PELS | GPIA_ASSOC | GPIT_MICRO );
    memset( &bmih, 0, sizeof bmih );
    bmih.cbFix = sizeof bmih; bmih.cx = 16; bmih.cy = 32; bmih.cPlanes = 1; bmih.cBitCount = 24;
    hbm = GpiCreateBitmap( mps, &bmih, 0, NULL, NULL );
    GpiSetBitmap( mps, hbm );
    GpiCreateLogColorTable( mps, 0, LCOLF_RGB, 0, 0, NULL );
    fill( mps, 0, 0, 16, 32, 0 );
    fill( mps, 0, 16, 8, 32, 0xFFFFFF );
    fill( mps, 0, 0, 4, 16, 0xFFFFFF );
    hptr = WinCreatePointer( HWND_DESKTOP, hbm, TRUE, 3, 4 );
    check( "WinCreatePointer", hptr != NULLHANDLE, TRUE );
    check( "WinQueryPointerInfo", WinQueryPointerInfo( hptr, &pi ), TRUE );
    check( "WinQueryPointerInfo hot spot", pi.xHotspot == 3 && pi.yHotspot == 4 && pi.hbmPointer == hbm, TRUE );

    hps = WinGetPS( hwndClient );
    GpiCreateLogColorTable( hps, 0, LCOLF_RGB, 0, 0, NULL );
    fill( hps, 100, 100, 140, 140, 0xFFFFFF );
    check( "WinDrawPointer", WinDrawPointer( hps, 100, 100, hptr, DP_NORMAL ), TRUE );
    check( "WinDrawPointer opaque", pel_rgb( hps, 110, 105 ), 0 );
    check( "WinDrawPointer transparent", pel_rgb( hps, 105, 105 ), 0xFFFFFF );
    fill( hps, 200, 100, 240, 140, 0 );
    WinDrawPointer( hps, 200, 100, hptr, DP_INVERTED );
    check( "WinDrawPointer inverted", pel_rgb( hps, 210, 105 ), 0xFFFFFF );
    WinReleasePS( hps );

    check( "WinDestroyPointer", WinDestroyPointer( hptr ), TRUE );
    check( "WinQueryPointerInfo destroyed", WinQueryPointerInfo( hptr, &pi ), FALSE );
    check( "WinQueryPointerInfo system", WinQueryPointerInfo( WinQuerySysPointer( HWND_DESKTOP, SPTR_ARROW, FALSE ), &pi ), TRUE );
    GpiSetBitmap( mps, NULLHANDLE );
    GpiDeleteBitmap( hbm );
    GpiDestroyPS( mps );
    DevCloseDC( hdc );
    check( "WinCreatePointer bad bitmap", WinCreatePointer( HWND_DESKTOP, 12345, TRUE, 0, 0 ), NULLHANDLE );
}

static void error_checks( void )
{
    ERRINFO32 *pe;
    PSZ      msg;

    WinGetLastError( hab );
    check( "WinGetErrorInfo none", WinGetErrorInfo( hab ) == NULL, TRUE );
    check( "WinSetErrorInfo", WinSetErrorInfo( 0x12345, SEI_NOPROMPT ), 0x12345 );
    check( "WinSetErrorInfo sets the last error", WinGetLastError( hab ), 0x12345 );
    WinSetErrorInfo( 0x12345, SEI_NOPROMPT );
    pe = (ERRINFO32 *)WinGetErrorInfo( hab );
    check( "WinGetErrorInfo", pe != NULL, TRUE );
    if( pe ) {
        msg = (PSZ)pe + *(PUSHORT)( (PBYTE)pe + pe->offaoffszMsg );
        check( "WinGetErrorInfo id", pe->idError == 0x12345 && pe->cDetailLevel == 1 && pe->cbFixedErrInfo == sizeof( ERRINFO32 ), TRUE );
        check( "WinGetErrorInfo message", msg[0] != '\0', TRUE );
        check( "WinFreeErrorInfo", WinFreeErrorInfo( (PERRINFO)pe ), TRUE );
        check( "WinFreeErrorInfo twice", WinFreeErrorInfo( (PERRINFO)pe ), FALSE );
    }
    check( "WinGetErrorInfo takes the error", WinGetLastError( hab ), 0 );
}

static void library_checks( void )
{
    HLIB  hlib = WinLoadLibrary( hab, "PMSHAPI" );
    PFNWP pfn;
    LONG  types[2] = { DTYP_LONG, DTYP_LONG };

    check( "WinLoadLibrary", hlib != NULLHANDLE, TRUE );
    pfn = WinLoadProcedure( hab, hlib, "PrfOpenProfile" );
    check( "WinLoadProcedure", pfn != NULL, TRUE );
    check( "WinLoadProcedure missing", WinLoadProcedure( hab, hlib, "NoSuchProcedure" ) == NULL, TRUE );
    check( "WinDeleteProcedure", WinDeleteProcedure( hab, pfn ), TRUE );
    check( "WinDeleteLibrary", WinDeleteLibrary( hab, hlib ), TRUE );
    check( "WinLoadLibrary missing", WinLoadLibrary( hab, "NOSUCHDL" ), NULLHANDLE );
    hlib = WinLoadLibrary( hab, "PMWIN" );
    check( "WinLoadProcedure from PMWIN", hlib != NULLHANDLE
           && WinLoadProcedure( hab, hlib, "WinQueryDesktopWindow" ) != NULL, TRUE );
    check( "WinRegisterUserMsg", WinRegisterUserMsg( hab, WM_USER + 50, DTYP_LONG, RUM_IN, DTYP_LONG, RUM_IN, DTYP_LONG ), TRUE );
    check( "WinRegisterUserDatatype", WinRegisterUserDatatype( hab, DTYP_USER + 1, 2, types ), TRUE );
    check( "WinRegisterUserDatatype below DTYP_USER", WinRegisterUserDatatype( hab, 5, 2, types ), FALSE );
}

static void position_checks( HWND hwndClient )
{
    SWP    swp;
    POINTL pt;

    check( "WinGetMaxPosition", WinGetMaxPosition( hwndFrame, &swp ), TRUE );
    check( "WinGetMaxPosition fills the SWP", ( swp.fl & SWP_MAXIMIZE ) && swp.cx > 0 && swp.cy > 0 && swp.hwnd == hwndFrame, TRUE );
    check( "WinGetMaxPosition not a frame", WinGetMaxPosition( hwndClient, &swp ), FALSE );
    check( "WinGetMinPosition", WinGetMinPosition( hwndFrame, &swp, NULL ), TRUE );
    check( "WinGetMinPosition fills the SWP", ( swp.fl & SWP_MINIMIZE ) && swp.cx > 0, TRUE );
    pt.x = 5; pt.y = 6;
    WinGetMinPosition( hwndFrame, &swp, &pt );
    check( "WinGetMinPosition at a point", swp.x == 5 && swp.y == 6, TRUE );
}

/* ---------- window positions in the profile ---------- */

static void store_checks( void )
{
    SWP   before, after;
    ULONG style;

    WinSetWindowPos( hwndFrame, HWND_TOP, 40, 50, 420, 300, SWP_MOVE | SWP_SIZE );
    WinQueryWindowPos( hwndFrame, &before );
    check( "WinStoreWindowPos", WinStoreWindowPos( "PMWinUtilTest", "Position", hwndFrame ), TRUE );
    WinSetWindowPos( hwndFrame, HWND_TOP, 10, 20, 200, 150, SWP_MOVE | SWP_SIZE );
    check( "WinRestoreWindowPos", WinRestoreWindowPos( "PMWinUtilTest", "Position", hwndFrame ), TRUE );
    WinQueryWindowPos( hwndFrame, &after );
    check( "WinRestoreWindowPos position", after.x == before.x && after.y == before.y
           && after.cx == before.cx && after.cy == before.cy, TRUE );
    check( "WinRestoreWindowPos unknown key", WinRestoreWindowPos( "PMWinUtilTest", "Nothing", hwndFrame ), FALSE );

    /* A maximized frame comes back maximized, and restores to its normal place. */
    WinSetWindowPos( hwndFrame, HWND_TOP, 0, 0, 0, 0, SWP_MAXIMIZE );
    WinStoreWindowPos( "PMWinUtilTest", "Maximized", hwndFrame );
    WinSetWindowPos( hwndFrame, HWND_TOP, 0, 0, 0, 0, SWP_RESTORE );
    WinSetWindowPos( hwndFrame, HWND_TOP, 15, 25, 250, 180, SWP_MOVE | SWP_SIZE );
    WinRestoreWindowPos( "PMWinUtilTest", "Maximized", hwndFrame );
    style = WinQueryWindowULong( hwndFrame, QWL_STYLE );
    check( "WinRestoreWindowPos maximized", ( style & WS_MAXIMIZED ) != 0, TRUE );
    WinSetWindowPos( hwndFrame, HWND_TOP, 0, 0, 0, 0, SWP_RESTORE );
    WinQueryWindowPos( hwndFrame, &after );
    check( "WinRestoreWindowPos normal position", after.x == before.x && after.cx == before.cx, TRUE );
}

/* ---------- the clipboard ---------- */

static void clipboard_checks( HWND hwndClient )
{
    QMSG  q;
    ULONG info = 0;

    WinOpenClipbrd( hab );
    WinEmptyClipbrd( hab );
    check( "WinSetClipbrdOwner", WinSetClipbrdOwner( hab, hwndClient ), TRUE );
    check( "WinQueryClipbrdOwner", WinQueryClipbrdOwner( hab ) == hwndClient, TRUE );
    check( "WinSetClipbrdViewer", WinSetClipbrdViewer( hab, hwndClient ), TRUE );
    check( "WinQueryClipbrdViewer", WinQueryClipbrdViewer( hab ) == hwndClient, TRUE );
    WinSetClipbrdData( hab, 1234, FMT_TEST, CFI_HANDLE );
    check( "WinEnumClipbrdFmts first", WinEnumClipbrdFmts( hab, 0 ), FMT_TEST );
    check( "WinEnumClipbrdFmts last", WinEnumClipbrdFmts( hab, FMT_TEST ), 0 );
    check( "WinQueryClipbrdFmtInfo", WinQueryClipbrdFmtInfo( hab, FMT_TEST, &info ) && info == CFI_HANDLE, TRUE );
    check( "WinQueryClipbrdFmtInfo absent", WinQueryClipbrdFmtInfo( hab, FMT_TEST + 1, &info ), FALSE );
    WinCloseClipbrd( hab );
    check( "WM_DRAWCLIPBOARD to the viewer",
           WinPeekMsg( hab, &q, hwndClient, WM_DRAWCLIPBOARD, WM_DRAWCLIPBOARD, PM_REMOVE ), TRUE );
    WinOpenClipbrd( hab );
    WinEmptyClipbrd( hab );
    WinCloseClipbrd( hab );
    check( "WM_DESTROYCLIPBOARD to the owner",
           WinPeekMsg( hab, &q, hwndClient, WM_DESTROYCLIPBOARD, WM_DESTROYCLIPBOARD, PM_REMOVE ), TRUE );
    check( "emptying ends the ownership", WinQueryClipbrdOwner( hab ), NULLHANDLE );
    WinSetClipbrdViewer( hab, NULLHANDLE );
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PAINT:
    {
        RECTL rcl;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        WinQueryWindowRect( hwnd, &rcl );
        WinFillRect( hps, &rcl, CLR_WHITE );
        WinEndPaint( hps );
        if( !checked ) {
            char line[64];
            checked = TRUE;
            string_checks();
            dialog_checks( hwnd );
            paint_checks( hwnd );
            clipboard_checks( hwnd );
            wait_checks();
            pointer_checks( hwnd );
            error_checks();
            library_checks();
            position_checks( hwnd );
            store_checks();
            append_long( append_str( append_long( append_str( line, PREFIX "done " ), n_passed ), "/" ), n_checks );
            strcat( line, "\r\n" );
            write_stdout( line );
        }
        return 0;
    }
    case WM_CLOSE:
        WinPostMsg( hwnd, WM_QUIT, 0, 0 );
        return 0;
    }
    return WinDefWindowProc( hwnd, msg, mp1, mp2 );
}

int main( void )
{
    HMQ   hmq;
    HWND  hwndClient;
    QMSG  qmsg;
    ULONG fl = FCF_TITLEBAR | FCF_SIZEBORDER | FCF_SYSMENU | FCF_TASKLIST;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "PMWinUtilTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &fl, "PMWinUtilTest", "PMWIN utility calls",
                                    WS_VISIBLE, NULLHANDLE, 0, &hwndClient );
    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );
    WinDestroyWindow( hwndFrame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
