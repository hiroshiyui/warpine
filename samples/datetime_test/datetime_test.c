/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * datetime_test.c - dates after 2038
 *
 * Checks the current date and time, setting it, file times and Warpine's
 * own 64-bit time calls on either side of 2038 (signed 32-bit time_t) and
 * 2106 (unsigned 32-bit seconds):
 *
 *   - DosGetDateTime agrees with DosQuerySysInfo(QSV_TIME_LOW/HIGH) once
 *     its time-zone field (minutes west of UTC) is applied
 *   - started with an expected year (the integration test starts the
 *     session at 2040-03-01 with WARPINE_CLOCK): DosGetDateTime, QSV_TIME
 *     and the C runtime's time() / localtime() are past 2038
 *   - DosSetDateTime to 2110-01-02 03:04:05: DosGetDateTime and
 *     QSV_TIME_HIGH follow; a child started afterwards sees 2110 too;
 *     invalid dates give ERROR_TS_DATETIME
 *   - DosSetFileInfo / DosSetPathInfo set file times up to 2107-12-31,
 *     read back by DosQueryFileInfo / DosQueryPathInfo / DosFindFirst;
 *     a date and time of 0 / 0 leave a time unchanged
 *   - WARPINE.DLL (DosLoadModule + DosQueryProcAddr by name):
 *     WrpTime64, WrpGetDateTime64, WrpLocalTime64, WrpMakeTime64
 *
 * A copy started as "child <year>" (the second string at pib_pchcmd,
 * never argv) ends with 0 when its DosGetDateTime year is <year>.
 *
 * Each check is written to stdout:
 *
 *   datetime_test: <name>: ok
 *   datetime_test: <name>: FAIL got <value> want <value>
 *
 * followed by "datetime_test: timezone=<minutes west>" and
 * "datetime_test: done <passed>/<total>"; the exit code is the number of
 * failed checks.
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>
#include <time.h>

#ifndef ERROR_TS_DATETIME
#define ERROR_TS_DATETIME 327
#endif

#define SET_YEAR     2110
#define SECS_PER_DAY 86400L
/* 2038-01-19 03:14:08 UTC and 2200-01-01 00:00 UTC. */
#define Y2038        2147483648LL
#define Y2200        7258118400LL
#define TEST_FILE    "C:\\DT_TEST.TMP"

typedef APIRET (APIENTRY *PFN_TIME64)( long long *ptime );
typedef APIRET (APIENTRY *PFN_GETDT64)( DATETIME *pdt, long long *ptime );
typedef APIRET (APIENTRY *PFN_LOCAL64)( const long long *ptime, DATETIME *pdt );
typedef APIRET (APIENTRY *PFN_MAKE64)( const DATETIME *pdt, long long *ptime );

static int n_checks = 0;
static int n_passed = 0;

/* ---------- output ---------- */

static char *append_str( char *p, const char *s )
{
    while( *s ) *p++ = *s++;
    *p = '\0';
    return p;
}

static char *append_ll( char *p, long long n )
{
    char tmp[24];
    int  i = 0;
    unsigned long long u = n < 0 ? (unsigned long long)-n : (unsigned long long)n;
    if( n < 0 ) *p++ = '-';
    do { tmp[i++] = (char)( '0' + (int)( u % 10 ) ); u /= 10; } while( u );
    while( i ) *p++ = tmp[--i];
    *p = '\0';
    return p;
}

static void write_stdout( const char *s )
{
    ULONG written;
    DosWrite( 1, (PVOID)s, strlen( s ), &written );
}

static void check( const char *what, long long got, long long want )
{
    char  line[200];
    char *p = append_str( line, "datetime_test: " );
    p = append_str( p, what );
    if( got == want ) {
        append_str( p, ": ok\r\n" );
        n_passed++;
    } else {
        p = append_str( p, ": FAIL got " );
        p = append_ll( p, got );
        p = append_str( p, " want " );
        p = append_ll( p, want );
        append_str( p, "\r\n" );
    }
    write_stdout( line );
    n_checks++;
}

/* got within [lo, hi]: reported as got == got, or got == the nearer end. */
static void check_range( const char *what, long long got, long long lo, long long hi )
{
    check( what, got, got < lo ? lo : got > hi ? hi : got );
}

/* ---------- dates ---------- */

/* Days from 1970-01-01 to y-m-d (proleptic Gregorian). */
static long long days_from_civil( long y, unsigned m, unsigned d )
{
    long     era;
    unsigned yoe, doy, doe;
    y -= m <= 2;
    era = ( y >= 0 ? y : y - 399 ) / 400;
    yoe = (unsigned)( y - era * 400 );
    doy = ( 153 * ( m + ( m > 2 ? -3 : 9 ) ) + 2 ) / 5 + d - 1;
    doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return (long long)era * 146097 + (long long)doe - 719468;
}

/* The UTC time a DATETIME names (its timezone is minutes west of UTC). */
static long long datetime_utc( const DATETIME *dt )
{
    long long local = days_from_civil( dt->year, dt->month, dt->day ) * SECS_PER_DAY
                    + dt->hours * 3600L + dt->minutes * 60L + dt->seconds;
    return local + dt->timezone * 60L;
}

static long long qsv_time( void )
{
    ULONG t[2];
    if( DosQuerySysInfo( QSV_TIME_LOW, QSV_TIME_HIGH, t, sizeof( t ) ) != NO_ERROR ) return -1;
    return ( (long long)t[1] << 32 ) | t[0];
}

static USHORT make_fdate( unsigned year, unsigned month, unsigned day )
{
    return (USHORT)( ( ( year - 1980 ) << 9 ) | ( month << 5 ) | day );
}

static USHORT make_ftime( unsigned hours, unsigned minutes, unsigned seconds )
{
    return (USHORT)( ( hours << 11 ) | ( minutes << 5 ) | ( seconds / 2 ) );
}

#define WORD_OF( x ) ( *(USHORT *)&( x ) )

/* ---------- processes ---------- */

/* The command line's arguments: the second string at pib_pchcmd. */
static const char *command_args( void )
{
    PTIB ptib;
    PPIB ppib;
    DosGetInfoBlocks( &ptib, &ppib );
    if( ppib->pib_pchcmd == NULL ) return "";
    return ppib->pib_pchcmd + strlen( ppib->pib_pchcmd ) + 1;
}

static long parse_long( const char *s )
{
    long n = 0;
    while( *s == ' ' ) s++;
    while( *s >= '0' && *s <= '9' ) n = n * 10 + ( *s++ - '0' );
    return n;
}

/* Start "child <year>" and wait: its exit code. */
static long run_child( unsigned year )
{
    PTIB        ptib;
    PPIB        ppib;
    char        me[CCHMAXPATH], args[CCHMAXPATH + 32], *p;
    RESULTCODES res;

    DosGetInfoBlocks( &ptib, &ppib );
    if( DosQueryModuleName( ppib->pib_hmte, sizeof( me ), me ) != NO_ERROR ) return -1;
    memset( args, 0, sizeof( args ) );
    p = append_str( args, me ) + 1;
    p = append_str( p, "child " );
    append_ll( p, year );
    if( DosExecPgm( NULL, 0, EXEC_SYNC, (PSZ)args, NULL, &res, (PSZ)me ) != NO_ERROR ) return -2;
    return (long)res.codeResult;
}

/* ---------- the checks ---------- */

static void check_clock( long start_year )
{
    DATETIME   dt;
    long long  qsv;
    time_t     t;
    struct tm *tm;
    char       line[64];

    check( "DosGetDateTime", DosGetDateTime( &dt ), NO_ERROR );
    qsv = qsv_time();
    check_range( "DosGetDateTime agrees with QSV_TIME", datetime_utc( &dt ) - qsv, -2, 2 );
    append_ll( append_str( line, "datetime_test: timezone=" ), dt.timezone );
    write_stdout( strcat( line, "\r\n" ) );
    if( start_year == 0 ) return;

    check( "start year from WARPINE_CLOCK", dt.year, start_year );
    check( "QSV_TIME past 2038", qsv >= Y2038, 1 );
    /* Open Watcom's time_t is unsigned long: good until 2106. */
    t = time( NULL );
    check( "time() past 2038 (unsigned)", (unsigned long)t >= 0x80000000UL, 1 );
    tm = localtime( &t );
    check( "localtime(time()) year", tm != NULL ? tm->tm_year + 1900 : 0, start_year );
}

static void check_set_date_time( void )
{
    DATETIME  dt, bad;
    ULONG     t[2];

    memset( &dt, 0, sizeof( dt ) );
    dt.year = SET_YEAR; dt.month = 1; dt.day = 2;
    dt.hours = 3; dt.minutes = 4; dt.seconds = 5;
    check( "DosSetDateTime 2110-01-02 03:04:05", DosSetDateTime( &dt ), NO_ERROR );
    memset( &dt, 0, sizeof( dt ) );
    DosGetDateTime( &dt );
    check( "DosGetDateTime year after set", dt.year, SET_YEAR );
    check( "DosGetDateTime month/day after set", dt.month * 100 + dt.day, 102 );
    check_range( "DosGetDateTime time after set",
                 dt.hours * 3600L + dt.minutes * 60L + dt.seconds, 3 * 3600L + 4 * 60L + 5,
                 3 * 3600L + 4 * 60L + 15 );
    check( "DosGetDateTime weekday (2110-01-02 is a Thursday)", dt.weekday, 4 );
    DosQuerySysInfo( QSV_TIME_LOW, QSV_TIME_HIGH, t, sizeof( t ) );
    check( "QSV_TIME_HIGH past 2106", t[1], 1 );
    check( "child started afterwards sees 2110", run_child( SET_YEAR ), 0 );

    bad = dt; bad.month = 13;
    check( "DosSetDateTime month 13", DosSetDateTime( &bad ), ERROR_TS_DATETIME );
    bad = dt; bad.month = 2; bad.day = 29; bad.year = 2111;
    check( "DosSetDateTime 2111-02-29", DosSetDateTime( &bad ), ERROR_TS_DATETIME );
    bad = dt; bad.hundredths = 100;
    check( "DosSetDateTime hundredths 100", DosSetDateTime( &bad ), ERROR_TS_DATETIME );
    bad = dt; bad.year = 1979;
    check( "DosSetDateTime 1979", DosSetDateTime( &bad ), ERROR_TS_DATETIME );
    DosGetDateTime( &dt );
    check( "a refused set leaves the clock", dt.year, SET_YEAR );
}

static void check_file_times( void )
{
    HFILE        hf;
    ULONG        action, count = 1;
    FILESTATUS3  fs, got;
    FILEFINDBUF3 find;
    HDIR         hdir = HDIR_CREATE;
    APIRET       rc;

    DosDelete( (PSZ)TEST_FILE );
    rc = DosOpen( (PSZ)TEST_FILE, &hf, &action, 0, FILE_NORMAL,
                  OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                  OPEN_ACCESS_READWRITE | OPEN_SHARE_DENYNONE, NULL );
    check( "DosOpen test file", rc, NO_ERROR );
    if( rc != NO_ERROR ) return;

    memset( &fs, 0, sizeof( fs ) );
    WORD_OF( fs.fdateLastWrite )  = make_fdate( 2099, 12, 31 );
    WORD_OF( fs.ftimeLastWrite )  = make_ftime( 23, 59, 58 );
    WORD_OF( fs.fdateLastAccess ) = make_fdate( 2050, 6, 15 );
    WORD_OF( fs.ftimeLastAccess ) = make_ftime( 10, 20, 30 );
    check( "DosSetFileInfo 2099-12-31", DosSetFileInfo( hf, FIL_STANDARD, &fs, sizeof( fs ) ), NO_ERROR );
    check( "DosQueryFileInfo", DosQueryFileInfo( hf, FIL_STANDARD, &got, sizeof( got ) ), NO_ERROR );
    check( "write date 2099-12-31", WORD_OF( got.fdateLastWrite ), make_fdate( 2099, 12, 31 ) );
    check( "write time 23:59:58", WORD_OF( got.ftimeLastWrite ), make_ftime( 23, 59, 58 ) );
    check( "access date 2050-06-15", WORD_OF( got.fdateLastAccess ), make_fdate( 2050, 6, 15 ) );
    check( "access time 10:20:30", WORD_OF( got.ftimeLastAccess ), make_ftime( 10, 20, 30 ) );

    /* 0 / 0 keeps the write time; the access time changes. */
    memset( &fs, 0, sizeof( fs ) );
    WORD_OF( fs.fdateLastAccess ) = make_fdate( 2040, 2, 29 );
    check( "DosSetFileInfo access only", DosSetFileInfo( hf, FIL_STANDARD, &fs, sizeof( fs ) ), NO_ERROR );
    DosQueryFileInfo( hf, FIL_STANDARD, &got, sizeof( got ) );
    check( "0 / 0 keeps the write date", WORD_OF( got.fdateLastWrite ), make_fdate( 2099, 12, 31 ) );
    check( "access date 2040-02-29", WORD_OF( got.fdateLastAccess ), make_fdate( 2040, 2, 29 ) );

    WORD_OF( fs.fdateLastAccess ) = make_fdate( 2041, 13, 1 );
    check( "DosSetFileInfo month 13", DosSetFileInfo( hf, FIL_STANDARD, &fs, sizeof( fs ) ),
           ERROR_INVALID_PARAMETER );
    check( "DosSetFileInfo short buffer", DosSetFileInfo( hf, FIL_STANDARD, &fs, 10 ),
           ERROR_BUFFER_OVERFLOW );
    DosClose( hf );

    memset( &fs, 0, sizeof( fs ) );
    WORD_OF( fs.fdateLastWrite ) = make_fdate( 2107, 12, 31 );
    WORD_OF( fs.ftimeLastWrite ) = make_ftime( 23, 59, 58 );
    check( "DosSetPathInfo 2107-12-31",
           DosSetPathInfo( (PSZ)TEST_FILE, FIL_STANDARD, &fs, sizeof( fs ), 0 ), NO_ERROR );
    check( "DosQueryPathInfo",
           DosQueryPathInfo( (PSZ)TEST_FILE, FIL_STANDARD, &got, sizeof( got ) ), NO_ERROR );
    check( "path write date 2107-12-31", WORD_OF( got.fdateLastWrite ), make_fdate( 2107, 12, 31 ) );
    rc = DosFindFirst( (PSZ)TEST_FILE, &hdir, FILE_NORMAL, &find, sizeof( find ), &count, FIL_STANDARD );
    check( "DosFindFirst", rc, NO_ERROR );
    check( "found write date 2107-12-31", WORD_OF( find.fdateLastWrite ), make_fdate( 2107, 12, 31 ) );
    if( rc == NO_ERROR ) DosFindClose( hdir );
    DosDelete( (PSZ)TEST_FILE );
}

static void check_warpine_dll( void )
{
    HMODULE     hmod;
    char        fail[32];
    PFN_TIME64  wrp_time64 = NULL;
    PFN_GETDT64 wrp_get_dt64 = NULL;
    PFN_LOCAL64 wrp_local64 = NULL;
    PFN_MAKE64  wrp_make64 = NULL;
    DATETIME    dt;
    long long   now, t, back;

    if( DosLoadModule( fail, sizeof( fail ), (PSZ)"WARPINE", &hmod ) != NO_ERROR ) {
        check( "DosLoadModule WARPINE (not on OS/2)", 1, 0 );
        return;
    }
    DosQueryProcAddr( hmod, 0, (PSZ)"WrpTime64", (PFN *)&wrp_time64 );
    DosQueryProcAddr( hmod, 0, (PSZ)"WrpGetDateTime64", (PFN *)&wrp_get_dt64 );
    DosQueryProcAddr( hmod, 0, (PSZ)"WrpLocalTime64", (PFN *)&wrp_local64 );
    DosQueryProcAddr( hmod, 0, (PSZ)"WrpMakeTime64", (PFN *)&wrp_make64 );
    check( "WARPINE.DLL entry points by name",
           wrp_time64 && wrp_get_dt64 && wrp_local64 && wrp_make64, 1 );
    if( !( wrp_time64 && wrp_get_dt64 && wrp_local64 && wrp_make64 ) ) return;

    check( "WrpTime64", wrp_time64( &now ), NO_ERROR );
    check_range( "WrpTime64 agrees with QSV_TIME", now - qsv_time(), -2, 2 );
    check( "WrpGetDateTime64", wrp_get_dt64( &dt, &t ), NO_ERROR );
    check( "WrpGetDateTime64 year", dt.year, SET_YEAR );
    check( "WrpGetDateTime64 forms agree", datetime_utc( &dt ) - t, 0 );

    t = Y2200;
    check( "WrpLocalTime64 2200", wrp_local64( &t, &dt ), NO_ERROR );
    check( "WrpLocalTime64 2200 agrees", datetime_utc( &dt ), Y2200 );
    check( "WrpMakeTime64 2200", wrp_make64( &dt, &back ), NO_ERROR );
    check( "WrpMakeTime64 round trip 2200", back, Y2200 );
    t = Y2038;
    wrp_local64( &t, &dt );
    check( "WrpLocalTime64 2^31 year", dt.year, 2038 );
    wrp_make64( &dt, &back );
    check( "WrpMakeTime64 round trip 2^31", back, Y2038 );
    dt.month = 2; dt.day = 30;
    check( "WrpMakeTime64 Feb 30", wrp_make64( &dt, &back ), ERROR_TS_DATETIME );
    check( "WrpTime64 NULL", wrp_time64( NULL ), ERROR_INVALID_PARAMETER );
    DosFreeModule( hmod );
}

int main( void )
{
    const char *args = command_args();
    char        line[64];

    if( strncmp( args, "child ", 6 ) == 0 ) {
        DATETIME dt;
        DosGetDateTime( &dt );
        return dt.year == parse_long( args + 6 ) ? 0 : 1;
    }

    check_clock( parse_long( args ) );
    check_set_date_time();
    check_file_times();
    check_warpine_dll();

    {
        char *p = append_str( line, "datetime_test: done " );
        p = append_ll( p, n_passed );
        p = append_str( p, "/" );
        p = append_ll( p, n_checks );
        append_str( p, "\r\n" );
        write_stdout( line );
    }
    return n_checks - n_passed;
}
