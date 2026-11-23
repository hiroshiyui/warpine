/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_audio_test.c - DosBeep and MMPM/2 waveaudio checks for Warpine
 *
 * Writes a 0.4 s, 8 kHz, 8-bit mono square-wave file (tone.wav) and then:
 *   - times DosBeep(880, 300), which must block for about 300 ms;
 *   - opens tone.wav with mciSendString and an alias, checks its mode,
 *     plays it with "wait" (about 400 ms) and sets its volume;
 *   - plays it again with "notify" and waits for MM_MCINOTIFY;
 *   - plays it through mciSendCommand (MCI_OPEN_PARMS / MCI_PLAY_PARMS)
 *     with MCI_WAIT.
 * Each check is written to stdout and shown in the window:
 *
 *   pm_audio_test: <name>: ok
 *   pm_audio_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_audio_test: done <passed>/<total>". Durations are checked
 * as lower bounds (playback must not return early) with a generous upper
 * bound, since audio output adds some latency.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSMISC
#define INCL_DOSFILEMGR
#define INCL_DOSPROCESS
#define INCL_OS2MM
#include <os2.h>
#include <os2me.h>
#include <string.h>

#define N_CHECKS_MAX 24
#define LINE_H       18
#define WAV_RATE     8000
#define WAV_MS       400
#define WAV_SAMPLES  ( WAV_RATE * WAV_MS / 1000 )
#define BEEP_MS      300
#define SLACK_MS     2000         /* upper bound on extra latency */
#define ID_TIMEOUT   1
#define TIMEOUT_MS   10000
#define WM_START     ( WM_USER + 1 )

static HAB   hab;
static int   n_checks = 0;
static int   n_passed = 0;
static BOOL  finished = FALSE;
static char  results[N_CHECKS_MAX][80];

/* ---------- helpers (no libc printf) ---------- */

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
    char line[128];
    char *p;

    if( n_checks >= N_CHECKS_MAX ) return;
    p = append_str( results[n_checks], name );
    if( got == want ) {
        append_str( p, ": ok" );
        n_passed++;
    } else {
        p = append_str( p, ": FAIL got " );
        p = append_long( p, got );
        p = append_str( p, " want " );
        append_long( p, want );
    }
    strcpy( line, "pm_audio_test: " );
    strcat( line, results[n_checks] );
    strcat( line, "\r\n" );
    write_stdout( line );
    n_checks++;
}

static ULONG now_ms( void )
{
    ULONG ms = 0;
    DosQuerySysInfo( QSV_MS_COUNT, QSV_MS_COUNT, &ms, sizeof( ms ) );
    return ms;
}

/* A duration is right if playback did not return early, nor hang. */
static BOOL took( ULONG ms, ULONG at_least )
{
    return ms + 50 >= at_least && ms <= at_least + SLACK_MS;
}

static void put_u32( BYTE *p, ULONG v )
{
    p[0] = (BYTE)v; p[1] = (BYTE)( v >> 8 ); p[2] = (BYTE)( v >> 16 ); p[3] = (BYTE)( v >> 24 );
}

static void put_u16( BYTE *p, USHORT v )
{
    p[0] = (BYTE)v; p[1] = (BYTE)( v >> 8 );
}

/* tone.wav: RIFF header, 16-byte fmt chunk, 8-bit unsigned PCM data. */
static BOOL write_wav( void )
{
    static BYTE file[44 + WAV_SAMPLES];
    HFILE  hf;
    ULONG  action, written, i;
    APIRET rc;

    memcpy( file, "RIFF", 4 );      put_u32( file + 4, 36 + WAV_SAMPLES );
    memcpy( file + 8, "WAVEfmt ", 8 );
    put_u32( file + 16, 16 );       put_u16( file + 20, 1 );   /* PCM */
    put_u16( file + 22, 1 );        put_u32( file + 24, WAV_RATE );
    put_u32( file + 28, WAV_RATE ); put_u16( file + 32, 1 );
    put_u16( file + 34, 8 );
    memcpy( file + 36, "data", 4 ); put_u32( file + 40, WAV_SAMPLES );
    for( i = 0; i < WAV_SAMPLES; i++ )
        file[44 + i] = ( i / 10 ) % 2 ? 0xC0 : 0x40;    /* 400 Hz square */

    rc = DosOpen( "tone.wav", &hf, &action, 0, FILE_NORMAL,
                  OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                  OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYNONE, NULL );
    if( rc ) return FALSE;
    DosWrite( hf, file, sizeof( file ), &written );
    DosClose( hf );
    return written == sizeof( file );
}

/* Given with "play notify", returned in MM_MCINOTIFY. */
#define USER_PARM 0x4321

static ULONG mci( const char *cmd, char *ret, HWND hwnd )
{
    static char scratch[64];
    if( !ret ) ret = scratch;
    ret[0] = '\0';
    return mciSendString( (PSZ)cmd, ret, 64, hwnd, 0 );
}

/* ---------- the checks ---------- */

/* Synchronous part; the notify check finishes in WM MM_MCINOTIFY. */
static void run_checks( HWND hwnd )
{
    char  ret[64];
    ULONG t;

    check( "write tone.wav", write_wav(), TRUE );

    t = now_ms();
    DosBeep( 880, BEEP_MS );
    check( "DosBeep blocks for its duration", took( now_ms() - t, BEEP_MS ), TRUE );

    check( "open with alias", mci( "open tone.wav type waveaudio alias tone wait", ret, 0 ), MCIERR_SUCCESS );
    check( "open returns the device ID", ret[0] != '\0' && ret[0] != '0', TRUE );
    mci( "status tone mode", ret, 0 );
    check( "mode before playing is stopped", strcmp( ret, "stopped" ), 0 );

    t = now_ms();
    check( "play wait", mci( "play tone wait", NULL, 0 ), MCIERR_SUCCESS );
    check( "play wait blocks for the file's length", took( now_ms() - t, WAV_MS ), TRUE );
    mci( "status tone mode", ret, 0 );
    check( "mode after play wait is stopped", strcmp( ret, "stopped" ), 0 );
    check( "set volume", mci( "set tone audio volume to 50", NULL, 0 ), MCIERR_SUCCESS );

    check( "play notify returns at once",
           mciSendString( (PSZ)"play tone notify", NULL, 0, hwnd, USER_PARM ), MCIERR_SUCCESS );
    mci( "status tone mode", ret, 0 );
    check( "mode while playing", strcmp( ret, "playing" ), 0 );
    WinStartTimer( hab, hwnd, ID_TIMEOUT, TIMEOUT_MS );
}

/* mciSendCommand with the toolkit's parameter blocks. */
static void run_command_checks( void )
{
    MCI_OPEN_PARMS open;
    MCI_PLAY_PARMS play;
    ULONG          t, rc;

    memset( &open, 0, sizeof( open ) );
    open.pszDeviceType  = (PSZ)MAKEULONG( MCI_DEVTYPE_WAVEFORM_AUDIO, 0 );
    open.pszElementName = "tone.wav";
    rc = mciSendCommand( 0, MCI_OPEN, MCI_WAIT | MCI_OPEN_TYPE_ID | MCI_OPEN_ELEMENT, &open, 0 );
    check( "MCI_OPEN by device type ID", rc, MCIERR_SUCCESS );

    memset( &play, 0, sizeof( play ) );
    t = now_ms();
    rc = mciSendCommand( open.usDeviceID, MCI_PLAY, MCI_WAIT, &play, 0 );
    check( "MCI_PLAY with MCI_WAIT", rc, MCIERR_SUCCESS );
    check( "MCI_PLAY blocks for the file's length", took( now_ms() - t, WAV_MS ), TRUE );
    check( "MCI_CLOSE", mciSendCommand( open.usDeviceID, MCI_CLOSE, MCI_WAIT, NULL, 0 ), MCIERR_SUCCESS );
}

static void finish( HWND hwnd )
{
    char text[64];

    if( finished ) return;
    finished = TRUE;
    WinStopTimer( hab, hwnd, ID_TIMEOUT );
    check( "close", mci( "close tone", NULL, 0 ), MCIERR_SUCCESS );
    run_command_checks();
    DosDelete( "tone.wav" );
    strcpy( text, "pm_audio_test: done " );
    append_long( text + strlen( text ), n_passed );
    strcat( text, "/" );
    append_long( text + strlen( text ), n_checks );
    strcat( text, "\r\n" );
    write_stdout( text );
    WinInvalidateRect( hwnd, NULL, FALSE );
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_START:
        run_checks( hwnd );
        WinInvalidateRect( hwnd, NULL, FALSE );
        return 0;
    case MM_MCINOTIFY:
        check( "MM_MCINOTIFY reports success", SHORT1FROMMP( mp1 ), MCI_NOTIFY_SUCCESSFUL );
        check( "  with the user parameter", SHORT2FROMMP( mp1 ), USER_PARM );
        check( "  for MCI_PLAY", SHORT2FROMMP( mp2 ), MCI_PLAY );
        finish( hwnd );
        return 0;
    case WM_TIMER:
        if( SHORT1FROMMP( mp1 ) != ID_TIMEOUT ) break;
        check( "MM_MCINOTIFY arrives", FALSE, TRUE );
        finish( hwnd );
        return 0;
    case WM_PAINT:
    {
        RECTL  rcl;
        POINTL pt;
        int    i;
        HPS    hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        WinQueryWindowRect( hwnd, &rcl );
        WinFillRect( hps, &rcl, CLR_WHITE );
        GpiSetColor( hps, CLR_BLACK );
        pt.x = 8;
        pt.y = rcl.yTop - LINE_H;
        for( i = 0; i < n_checks; i++ ) {
            GpiCharStringAt( hps, &pt, strlen( results[i] ), results[i] );
            pt.y -= LINE_H;
        }
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
    HMQ   hmq;
    HWND  hwndFrame, hwndClient;
    QMSG  qmsg;
    ULONG flFrameFlags = FCF_TITLEBAR | FCF_SIZEBORDER | FCF_MINMAX |
                         FCF_SYSMENU | FCF_TASKLIST;

    hab = WinInitialize( 0 );
    if( hab == 0 ) return 1;
    hmq = WinCreateMsgQueue( hab, 0 );
    if( hmq == 0 ) {
        WinTerminate( hab );
        return 1;
    }

    WinRegisterClass( hab, "PMAudioTest", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flFrameFlags,
                                    "PMAudioTest", "Warpine audio test",
                                    0, NULLHANDLE, 0, &hwndClient );
    if( hwndFrame == 0 ) {
        WinDestroyMsgQueue( hmq );
        WinTerminate( hab );
        return 1;
    }
    WinPostMsg( hwndClient, WM_START, 0, 0 );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( hwndFrame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
