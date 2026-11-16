/*
 * pm_bench.c - self-benchmarking PM application for Warpine
 *
 * Runs a fixed series of measurements as soon as its window opens, shows
 * the results in the window and writes them to stdout, one line each:
 *
 *   pm_bench: <name> <ops> ops in <ms> ms = <ops/s> ops/s, <us>.<t> us/op
 *
 * followed by "pm_bench: done". Press R in the window to run again.
 *
 * Measurements (each runs for about BENCH_MS milliseconds):
 *   GpiBox      filled boxes drawn inside one WM_PAINT
 *   GpiLine     lines drawn inside one WM_PAINT
 *   GpiText     GpiCharStringAt calls inside one WM_PAINT
 *   WinSendMsg  synchronous messages to the own window procedure
 *   WinPostMsg  post-to-self round trips through the message queue
 *   Paint       WinInvalidateRect -> WM_PAINT -> WinEndPaint cycles
 *   Timer       WM_TIMER rate for a 10 ms WinStartTimer
 *
 * The clock is DosQuerySysInfo(QSV_MS_COUNT). The numbers are what the
 * application sees: under Warpine, GPI calls queue drawing for the GUI
 * thread, so the GPI figures are the cost of issuing the calls, not of
 * rasterising them. Use RUST_LOG=warpine::gui=debug for the host side.
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_DOSMISC
#define INCL_DOSFILEMGR
#include <os2.h>
#include <string.h>

#define BENCH_MS        300     /* duration of each measurement */
#define TIMER_BENCH_MS  1000    /* the timer test runs longer: ticks are slow */
#define TIMER_PERIOD_MS 10
#define ID_BENCH_TIMER  1
#define GPI_BATCH       64      /* GPI calls between clock reads */

#define WM_BENCH_NEXT   (WM_USER + 1)   /* start the next measurement */
#define WM_BENCH_PING   (WM_USER + 2)   /* one WinPostMsg round trip */
#define WM_BENCH_NOP    (WM_USER + 3)   /* WinSendMsg target */

enum {
    PH_IDLE, PH_BOX, PH_LINE, PH_TEXT, PH_SEND, PH_POST, PH_PAINT, PH_TIMER,
    PH_DONE
};
#define N_BENCH (PH_DONE - PH_BOX)

static const char *bench_name[N_BENCH] = {
    "GpiBox", "GpiLine", "GpiText", "WinSendMsg", "WinPostMsg", "Paint",
    "Timer"
};

static HAB   hab;
static int   phase = PH_IDLE;
static ULONG phase_start;               /* QSV_MS_COUNT at phase start */
static ULONG phase_ops;
static ULONG result_ops[N_BENCH];
static ULONG result_ms[N_BENCH];
static ULONG rng = 12345;

/* ---------- helpers (no libc printf) ---------- */

static ULONG now_ms( void )
{
    ULONG ms = 0;
    DosQuerySysInfo( QSV_MS_COUNT, QSV_MS_COUNT, &ms, sizeof( ms ) );
    return ms;
}

static ULONG elapsed_ms( void )
{
    return now_ms() - phase_start;      /* unsigned: correct across wrap */
}

static ULONG next_rand( ULONG limit )
{
    rng = rng * 1103515245UL + 12345UL;
    return ( rng >> 16 ) % ( limit ? limit : 1 );
}

static char *append_str( char *p, const char *s )
{
    while( *s ) *p++ = *s++;
    *p = '\0';
    return p;
}

static char *append_uint( char *p, ULONG n )
{
    char tmp[12];
    int  i = 0;
    do { tmp[i++] = (char)( '0' + n % 10 ); n /= 10; } while( n );
    while( i ) *p++ = tmp[--i];
    *p = '\0';
    return p;
}

/* "<ops> ops in <ms> ms = <rate> ops/s, <us>.<t> us/op" */
static void format_result( char *p, int i )
{
    ULONG ops = result_ops[i], ms = result_ms[i];
    ULONG rate, tenth_us;

    if( ms == 0 ) ms = 1;               /* sub-millisecond: report as 1 ms */
    rate     = ops <= 4000000UL ? ops * 1000UL / ms : ops / ms * 1000UL;
    tenth_us = ops ? ms * 10000UL / ops : 0;

    p = append_str( p, bench_name[i] );
    p = append_str( p, ": " );
    p = append_uint( p, ops );
    p = append_str( p, " ops in " );
    p = append_uint( p, result_ms[i] );
    p = append_str( p, " ms = " );
    p = append_uint( p, rate );
    p = append_str( p, " ops/s, " );
    p = append_uint( p, tenth_us / 10 );
    p = append_str( p, "." );
    p = append_uint( p, tenth_us % 10 );
    p = append_str( p, " us/op" );
}

static void write_stdout( const char *s )
{
    ULONG written;
    DosWrite( 1, (PVOID)s, strlen( s ), &written );
}

static void report_results( void )
{
    char line[128];
    int  i;

    for( i = 0; i < N_BENCH; i++ ) {
        strcpy( line, "pm_bench: " );
        format_result( line + strlen( line ), i );
        strcat( line, "\r\n" );
        write_stdout( line );
    }
    write_stdout( "pm_bench: done\r\n" );
}

/* ---------- measurements ---------- */

static void begin_phase( int ph )
{
    phase       = ph;
    phase_ops   = 0;
    phase_start = now_ms();
}

/* Record the running measurement and queue the next one. */
static void end_phase( HWND hwnd )
{
    result_ops[phase - PH_BOX] = phase_ops;
    result_ms[phase - PH_BOX]  = elapsed_ms();
    WinPostMsg( hwnd, WM_BENCH_NEXT, 0, 0 );
}

/* GpiBox / GpiLine / GpiText inside the current paint. */
static void run_gpi_phase( HPS hps, RECTL *rcl )
{
    LONG   w = rcl->xRight > 1 ? rcl->xRight : 1;
    LONG   h = rcl->yTop   > 1 ? rcl->yTop   : 1;
    POINTL pt;
    int    k;

    WinFillRect( hps, rcl, CLR_WHITE );
    do {
        for( k = 0; k < GPI_BATCH; k++ ) {
            GpiSetColor( hps, 1 + (LONG)next_rand( 15 ) );
            pt.x = (LONG)next_rand( w );
            pt.y = (LONG)next_rand( h );
            switch( phase ) {
            case PH_BOX:
                GpiMove( hps, &pt );
                pt.x += 8 + (LONG)next_rand( 40 );
                pt.y += 8 + (LONG)next_rand( 40 );
                GpiBox( hps, DRO_FILL, &pt, 0, 0 );
                break;
            case PH_LINE:
                GpiMove( hps, &pt );
                pt.x = (LONG)next_rand( w );
                pt.y = (LONG)next_rand( h );
                GpiLine( hps, &pt );
                break;
            default:
                GpiCharStringAt( hps, &pt, 19, "The quick brown fox" );
                break;
            }
        }
        phase_ops += GPI_BATCH;
    } while( elapsed_ms() < BENCH_MS );
}

static void draw_results( HPS hps, RECTL *rcl )
{
    POINTL pt;
    char   line[128];
    int    i;

    WinFillRect( hps, rcl, CLR_WHITE );
    GpiSetColor( hps, CLR_BLACK );
    pt.x = 16;
    pt.y = rcl->yTop - 28;

    if( phase != PH_DONE ) {
        GpiCharStringAt( hps, &pt, 11, "Running ..." );
        return;
    }
    GpiCharStringAt( hps, &pt, 33, "Warpine PM self-benchmark results" );
    for( i = 0; i < N_BENCH; i++ ) {
        pt.y -= 22;
        format_result( line, i );
        GpiCharStringAt( hps, &pt, strlen( line ), line );
    }
    pt.y -= 34;
    GpiSetColor( hps, CLR_DARKGRAY );
    GpiCharStringAt( hps, &pt, 20, "Press R to run again" );
}

/* Start the measurement after the current one, or finish. */
static void next_phase( HWND hwnd )
{
    int ph = phase == PH_IDLE || phase == PH_DONE ? PH_BOX : phase + 1;

    if( ph == PH_DONE ) {
        phase = PH_DONE;
        report_results();
        WinInvalidateRect( hwnd, NULL, FALSE );
        return;
    }
    begin_phase( ph );
    switch( ph ) {
    case PH_BOX:
    case PH_LINE:
    case PH_TEXT:
    case PH_PAINT:
        WinInvalidateRect( hwnd, NULL, FALSE );     /* measured in WM_PAINT */
        break;
    case PH_SEND:
        do {
            WinSendMsg( hwnd, WM_BENCH_NOP, 0, 0 );
            phase_ops++;
        } while( elapsed_ms() < BENCH_MS );
        end_phase( hwnd );
        break;
    case PH_POST:
        WinPostMsg( hwnd, WM_BENCH_PING, 0, 0 );
        break;
    case PH_TIMER:
        WinStartTimer( hab, hwnd, ID_BENCH_TIMER, TIMER_PERIOD_MS );
        break;
    }
}

MRESULT EXPENTRY ClientWndProc( HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2 )
{
    switch( msg ) {
    case WM_PAINT:
    {
        RECTL rcl;
        HPS   hps = WinBeginPaint( hwnd, NULLHANDLE, NULL );
        WinQueryWindowRect( hwnd, &rcl );
        switch( phase ) {
        case PH_BOX:
        case PH_LINE:
        case PH_TEXT:
            run_gpi_phase( hps, &rcl );
            WinEndPaint( hps );
            end_phase( hwnd );
            return 0;
        case PH_PAINT:
            /* A small box that moves each frame, so every paint differs. */
            {
                POINTL pt;
                GpiSetColor( hps, (phase_ops & 1) ? CLR_BLUE : CLR_RED );
                pt.x = (LONG)( phase_ops % 32 ) * 8;
                pt.y = 8;
                GpiMove( hps, &pt );
                pt.x += 16;
                pt.y += 16;
                GpiBox( hps, DRO_FILL, &pt, 0, 0 );
            }
            WinEndPaint( hps );
            phase_ops++;
            if( elapsed_ms() < BENCH_MS )
                WinInvalidateRect( hwnd, NULL, FALSE );
            else
                end_phase( hwnd );
            return 0;
        default:
            draw_results( hps, &rcl );
            WinEndPaint( hps );
            return 0;
        }
    }
    case WM_BENCH_NEXT:
        next_phase( hwnd );
        return 0;
    case WM_BENCH_NOP:
        return 0;
    case WM_BENCH_PING:
        if( phase != PH_POST ) return 0;
        phase_ops++;
        if( elapsed_ms() < BENCH_MS )
            WinPostMsg( hwnd, WM_BENCH_PING, 0, 0 );
        else
            end_phase( hwnd );
        return 0;
    case WM_TIMER:
        if( phase != PH_TIMER || SHORT1FROMMP( mp1 ) != ID_BENCH_TIMER )
            break;
        phase_ops++;
        if( elapsed_ms() >= TIMER_BENCH_MS ) {
            WinStopTimer( hab, hwnd, ID_BENCH_TIMER );
            end_phase( hwnd );
        }
        return 0;
    case WM_CHAR:
    {
        USHORT fs = SHORT1FROMMP( mp1 );
        USHORT ch = SHORT1FROMMP( mp2 );
        if( ( fs & KC_CHAR ) && !( fs & KC_KEYUP ) &&
            ( ch == 'r' || ch == 'R' ) && phase == PH_DONE ) {
            phase = PH_IDLE;
            WinInvalidateRect( hwnd, NULL, FALSE );
            WinPostMsg( hwnd, WM_BENCH_NEXT, 0, 0 );
            return (MRESULT)TRUE;
        }
        break;
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

    WinRegisterClass( hab, "PMBench", ClientWndProc, CS_SIZEREDRAW, 0 );
    hwndFrame = WinCreateStdWindow( HWND_DESKTOP, WS_VISIBLE, &flFrameFlags,
                                    "PMBench", "Warpine PM Benchmark",
                                    0, NULLHANDLE, 0, &hwndClient );
    if( hwndFrame == 0 ) {
        WinDestroyMsgQueue( hmq );
        WinTerminate( hab );
        return 1;
    }

    WinPostMsg( hwndClient, WM_BENCH_NEXT, 0, 0 );

    while( WinGetMsg( hab, &qmsg, NULLHANDLE, 0, 0 ) )
        WinDispatchMsg( hab, &qmsg );

    WinDestroyWindow( hwndFrame );
    WinDestroyMsgQueue( hmq );
    WinTerminate( hab );
    return 0;
}
