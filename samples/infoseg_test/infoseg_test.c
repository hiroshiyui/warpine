/*
 * infoseg_test.c - DosGetInfoSeg: the global and local information segments
 *
 * DosGetInfoSeg (DOSCALLS 8, the 16-bit DOS16GETINFOSEG) gives a selector
 * of the GINFOSEG (time and date, version, scheduler values) and one of the
 * LINFOSEG (process and thread ids). The program calls the entry point
 * directly with 32-bit pointers, as Warpine accepts it (on OS/2 a 32-bit
 * program goes through a 16-bit thunk), and turns the selectors into flat
 * pointers with DosSelToFlat. Checks:
 *
 *   - both selectors address a segment at offset 0
 *   - GINFOSEG time, date and time zone agree with DosGetDateTime and
 *     QSV_TIME_LOW, the version with QSV_VERSION_MAJOR / MINOR
 *   - the time fields move on by themselves (no second call) while the
 *     program sleeps
 *   - LINFOSEG pid / parent pid / thread id agree with the PIB and TIB
 *
 * Output: "infoseg_test: <name>: ok" or "... FAIL got <v> want <v>", then
 * "infoseg_test: done <passed>/<total>"; the exit code is the number of
 * failed checks.
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

/* The 16-bit entry, called directly (see above). */
extern USHORT APIENTRY DOS16GETINFOSEG( USHORT *pselGlobal, USHORT *pselLocal );
/* DOSCALLS 426: a 16:16 address (selector in the high word) as a flat one. */
extern ULONG APIENTRY DosSelToFlat( ULONG sel_off );

#pragma pack( 1 )
typedef struct {                /* OS/2 1.x toolkit GINFOSEG */
    ULONG  time;
    ULONG  msecs;
    UCHAR  hour, minutes, seconds, hundredths;
    SHORT  timezone;
    USHORT cusecTimerInterval;
    UCHAR  day, month;
    USHORT year;
    UCHAR  weekday;
    UCHAR  uchMajorVersion, uchMinorVersion, chRevisionLetter;
    UCHAR  sgCurrent, sgMax, cHugeShift, fProtectModeOnly;
    USHORT pidForeground;
    UCHAR  fDynamicSched, csecMaxWait;
    USHORT cmsecMinSlice, cmsecMaxSlice, bootdrive;
    UCHAR  amecRAS[32];
    UCHAR  csgWindowableVioMax, csgPMMax;
} GINFO;

typedef struct {                /* OS/2 1.x toolkit LINFOSEG */
    USHORT pidCurrent, pidParent, prtyCurrent, tidCurrent, sgCurrent;
    UCHAR  rfProcStatus, dummy1;
    USHORT fForeground;
    UCHAR  typeProcess, dummy2;
    USHORT selEnvironment, offCmdLine, cbDataSegment, cbStack, cbHeap, hmod, selDS;
} LINFO;
#pragma pack()

static int n_checks = 0;
static int n_passed = 0;

static char *append_str( char *p, const char *s )
{
    while( *s ) *p++ = *s++;
    *p = '\0';
    return p;
}

static char *append_long( char *p, long n )
{
    char tmp[12];
    int  i = 0;
    unsigned long u = n < 0 ? (unsigned long)-n : (unsigned long)n;
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

static void check( const char *what, long got, long want )
{
    char  line[160];
    char *p = append_str( append_str( line, "infoseg_test: " ), what );
    if( got == want ) {
        append_str( p, ": ok\r\n" );
        n_passed++;
    } else {
        p = append_long( append_str( p, ": FAIL got " ), got );
        append_str( append_long( append_str( p, " want " ), want ), "\r\n" );
    }
    write_stdout( line );
    n_checks++;
}

/* Days from 1970-01-01 to y-m-d. */
static long days_from_civil( long y, unsigned m, unsigned d )
{
    long     era;
    unsigned yoe, doy, doe;
    y -= m <= 2;
    era = ( y >= 0 ? y : y - 399 ) / 400;
    yoe = (unsigned)( y - era * 400 );
    doy = ( 153 * ( m + ( m > 2 ? -3 : 9 ) ) + 2 ) / 5 + d - 1;
    doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return era * 146097L + (long)doe - 719468L;
}

int main( void )
{
    USHORT   sel_global = 0, sel_local = 0;
    GINFO   *g;
    LINFO   *l;
    DATETIME dt;
    ULONG    qsv[3], t0;
    PTIB     ptib;
    PPIB     ppib;
    long     utc;
    char     line[48];

    check( "DosGetInfoSeg", DOS16GETINFOSEG( &sel_global, &sel_local ), NO_ERROR );
    check( "selectors given", sel_global != 0 && sel_local != 0 && sel_global != sel_local, 1 );
    g = (GINFO *)DosSelToFlat( (ULONG)sel_global << 16 );
    l = (LINFO *)DosSelToFlat( (ULONG)sel_local << 16 );
    check( "segments at offset 0", ( (ULONG)g & 0xFFFF ) == 0 && ( (ULONG)l & 0xFFFF ) == 0, 1 );

    DosGetDateTime( &dt );
    DosQuerySysInfo( QSV_TIME_LOW, QSV_TIME_LOW, &qsv[0], sizeof( ULONG ) );
    DosQuerySysInfo( QSV_VERSION_MAJOR, QSV_VERSION_MINOR, &qsv[1], 2 * sizeof( ULONG ) );
    check( "year", g->year, dt.year );
    check( "month", g->month, dt.month );
    check( "time zone", g->timezone, dt.timezone );
    check( "version", g->uchMajorVersion * 100 + g->uchMinorVersion, qsv[1] * 100 + qsv[2] );
    check( "time agrees with QSV_TIME_LOW", (long)( g->time - qsv[0] ) >= -2 && (long)( g->time - qsv[0] ) <= 2, 1 );
    /* hour:minutes:seconds in local time, time zone minutes west of UTC. */
    utc = (long)( (long long)days_from_civil( g->year, g->month, g->day ) * 86400LL
        + g->hour * 3600L + g->minutes * 60L + g->seconds + g->timezone * 60L - (long long)g->time );
    check( "local time and time zone agree with the UTC time", utc >= -2 && utc <= 2, 1 );
    check( "timer interval (tenths of a ms)", g->cusecTimerInterval, 320 );
    check( "boot drive", g->bootdrive, 3 );

    t0 = g->msecs;
    DosSleep( 300 );
    check( "msecs move on without another call", g->msecs - t0 >= 200 && g->msecs - t0 < 5000, 1 );

    DosGetInfoBlocks( &ptib, &ppib );
    check( "pid", l->pidCurrent, ppib->pib_ulpid );
    check( "parent pid", l->pidParent, ppib->pib_ulppid );
    check( "thread id", l->tidCurrent, ptib->tib_ptib2->tib2_ultid );
    check( "process type", l->typeProcess, ppib->pib_ultype );
    check( "foreground pid", g->pidForeground, ppib->pib_ulpid );

    append_str( append_long( append_str( append_long( append_str( line, "infoseg_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    return n_checks - n_passed;
}
