/*
 * pm_font_test.c - PM font selection and CJK text demo for Warpine
 *
 * Exercises: GpiCreateLogFont (face name, bold/italic, size), GpiSetCharSet,
 *   GpiDeleteSetId, GpiQueryTextBox, GpiQueryFontMetrics, GpiCharStringAt,
 *   and GpiSetCp with DBCS text in CP932 / CP936 / CP949 / CP950 (GPI text is in
 *   the codepage of its presentation space, not the process codepage).
 *
 * Each line of text is underlined with a rule whose length comes from
 * GpiQueryTextBox, so a mismatch between reported and drawn width is visible:
 * the rule must end exactly where the text ends.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSNLS
#include <os2.h>
#include <string.h>

#define LCID_BASE 10L

typedef struct {
    const char *face;      /* FATTRS szFacename ("" = system default)   */
    USHORT      selection; /* FATTR_SEL_* flags                         */
    LONG        height;    /* lMaxBaselineExt in pixels (0 = default)   */
    ULONG       codepage;  /* process codepage the text is encoded in   */
    const char *text;
} FONTLINE;

/* DBCS strings are spelled as byte escapes so the source stays pure ASCII. */
static const FONTLINE lines[] = {
    { "",                0,                                  0, 850,
      "System default: The quick brown fox jumps over the lazy dog" },
    { "Helv",            FATTR_SEL_BOLD,                     0, 850,
      "Helv Bold: The quick brown fox jumps over the lazy dog" },
    { "Courier",         0,                                  0, 850,
      "Courier: fixed pitch  iiii WWWW 0123456789" },
    { "Tms Rmn",         FATTR_SEL_ITALIC,                  24, 850,
      "Tms Rmn Italic 24: Presentation Manager" },
    { "Times New Roman", FATTR_SEL_BOLD | FATTR_SEL_ITALIC, 32, 850,
      "Bold Italic 32" },
    { "Helv",            0,                                  0, 932,   /* Japanese */
      "CP932: \x93\xFA\x96\x7B\x8C\xEA\x20\x82\xB1\x82\xF1\x82\xC9\x82\xBF\x82\xCD" },
    { "Helv",            0,                                  0, 936,   /* Simplified Chinese */
      "CP936: \xBC\xF2\xCC\xE5\xD6\xD0\xCE\xC4\x20\xC4\xE3\xBA\xC3" },
    { "Helv",            0,                                  0, 950,   /* Traditional Chinese */
      "CP950: \xC1\x63\xC5\xE9\xA4\xA4\xA4\xE5\x20\xA7\x41\xA6\x6E" },
    { "Helv",            0,                                  0, 949,   /* Korean */
      "CP949: \xC7\xD1\xB1\xB9\xBE\xEE\x20\xBE\xC8\xB3\xE7\xC7\xCF\xBC\xBC\xBF\xE4" },
    { "Helv",            0,                                 24, 932,
      "24 px: \x93\xFA\x96\x7B\x8C\xEA ABC" },
};
#define LINE_COUNT (sizeof(lines) / sizeof(lines[0]))

static void draw_scene( HWND hwnd, HPS hps )
{
    RECTL  rcl;
    POINTL pt;
    LONG   y;
    ULONG  i;

    WinQueryWindowRect( hwnd, &rcl );
    WinFillRect( hps, &rcl, CLR_WHITE );
    y = rcl.yTop - 10;

    for( i = 0; i < LINE_COUNT; i++ ) {
        FATTRS      fat;
        FONTMETRICS fm;
        POINTL      box[TXTBOX_COUNT];
        LONG        lcid = LCID_BASE + (LONG)i;
        LONG        len  = (LONG)strlen( lines[i].text );

        GpiSetCp( hps, lines[i].codepage );

        memset( &fat, 0, sizeof(fat) );
        fat.usRecordLength  = sizeof(fat);
        fat.fsSelection     = lines[i].selection;
        fat.lMaxBaselineExt = lines[i].height;
        strcpy( fat.szFacename, lines[i].face );
        GpiCreateLogFont( hps, NULL, lcid, &fat );
        GpiSetCharSet( hps, lcid );

        /* Step down by this font's line height, then draw on its baseline. */
        GpiQueryFontMetrics( hps, sizeof(fm), &fm );
        y -= fm.lMaxBaselineExt + 6;

        GpiSetColor( hps, CLR_BLACK );
        pt.x = 16;
        pt.y = y + fm.lMaxDescender;
        GpiCharStringAt( hps, &pt, len, (PCH)lines[i].text );

        /* Rule under the text, as long as GpiQueryTextBox says the text is. */
        GpiQueryTextBox( hps, len, (PCH)lines[i].text, TXTBOX_COUNT, box );
        GpiSetColor( hps, CLR_RED );
        pt.x = 16;
        pt.y = y - 1;
        GpiMove( hps, &pt );
        pt.x = 16 + box[TXTBOX_CONCAT].x;
        GpiLine( hps, &pt );

        GpiSetCharSet( hps, LCID_DEFAULT );
        GpiDeleteSetId( hps, lcid );
    }

}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PAINT:
    {
        HPS hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        draw_scene( hwnd, hps );
        WinEndPaint( hps );
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
    HAB   hab;
    HMQ   hmq;
    HWND  hwndFrame, hwndClient;
    QMSG  qmsg;
    ULONG flFrameFlags = FCF_TITLEBAR | FCF_SIZEBORDER | FCF_MINMAX |
                         FCF_SYSMENU | FCF_TASKLIST;

    hab = WinInitialize( 0 );
    if( hab == 0 ) return 1;
    hmq = WinCreateMsgQueue( hab, 0 );
    if( hmq == 0 ) { WinTerminate( hab ); return 1; }

    WinRegisterClass( hab, "PMFontTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow(
        HWND_DESKTOP, WS_VISIBLE,
        &flFrameFlags, "PMFontTest",
        "PM Font Test - IBM Plex",
        0, NULLHANDLE, 0, &hwndClient );
    if( hwndFrame == 0 ) {
        WinDestroyMsgQueue( hmq );
        WinTerminate( hab );
        return 1;
    }

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( hwndFrame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
