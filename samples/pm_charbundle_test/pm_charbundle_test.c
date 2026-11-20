/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * pm_charbundle_test.c - GpiSetAttrs / GpiQueryAttrs with a CHARBUNDLE
 *
 * The character bundle's mode, angle, shear, direction, text alignment,
 * extra and break extra are the attributes GpiSetCharMode ...
 * GpiSetCharBreakExtra set. The program sets them through GpiSetAttrs
 * (PRIM_CHAR) and reads them back with the single-attribute queries, sets
 * them with the single calls and reads them back with GpiQueryAttrs (which
 * returns the mask of the attributes still at their default), resets them
 * with the default mask, and has invalid values rejected without changing
 * anything. Each check is written to stdout:
 *
 *   pm_charbundle_test: <name>: ok
 *   pm_charbundle_test: <name>: FAIL got <value> want <value>
 *
 * followed by "pm_charbundle_test: done <passed>/<total>".
 */
#define INCL_WIN
#define INCL_GPI
#define INCL_GPIERRORS
#define INCL_DOSPROCESS
#include <os2.h>
#include <string.h>

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
    char  line[200], *p;
    p = append_str( line, "pm_charbundle_test: " );
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

#define TEXT_BITS ( CBB_MODE | CBB_ANGLE | CBB_SHEAR | CBB_DIRECTION | CBB_TEXT_ALIGN \
                    | CBB_EXTRA | CBB_BREAK_EXTRA )

static void run( HPS hps )
{
    CHARBUNDLE cb;
    GRADIENTL  angle;
    POINTL     shear;
    FIXED      fx;
    LONG       h, v;

    /* A fresh presentation space: every text attribute is at its default. */
    memset( &cb, 0x5A, sizeof( cb ) );
    check( "fresh: GpiQueryAttrs default mask", GpiQueryAttrs( hps, PRIM_CHAR, TEXT_BITS, &cb ), TEXT_BITS );
    check( "fresh: usPrecision (mode)", cb.usPrecision, CM_MODE1 );
    check( "fresh: angle x", cb.ptlAngle.x, 1 );
    check( "fresh: angle y", cb.ptlAngle.y, 0 );
    check( "fresh: shear y", cb.ptlShear.y, 1 );
    check( "fresh: direction", cb.usDirection, CHDIRN_LEFTRIGHT );
    check( "fresh: alignment", cb.usTextAlign, TA_NORMAL_HORIZ | TA_NORMAL_VERT );
    check( "fresh: extra", cb.fxExtra, 0 );

    /* GpiSetAttrs sets them all; the single queries see them. */
    cb.usPrecision = CM_MODE3;
    cb.ptlAngle.x = 1; cb.ptlAngle.y = 1;
    cb.ptlShear.x = 1; cb.ptlShear.y = 2;
    cb.usDirection = CHDIRN_TOPBOTTOM;
    cb.usTextAlign = TA_CENTER | TA_HALF;
    cb.fxExtra = MAKEFIXED( 3, 0 );
    cb.fxBreakExtra = MAKEFIXED( -2, 0 );
    check( "GpiSetAttrs(PRIM_CHAR)", GpiSetAttrs( hps, PRIM_CHAR, TEXT_BITS, 0, &cb ), TRUE );
    check( "GpiQueryCharMode", GpiQueryCharMode( hps ), CM_MODE3 );
    GpiQueryCharAngle( hps, &angle );
    check( "GpiQueryCharAngle x", angle.x, 1 );
    check( "GpiQueryCharAngle y", angle.y, 1 );
    GpiQueryCharShear( hps, &shear );
    check( "GpiQueryCharShear x", shear.x, 1 );
    check( "GpiQueryCharShear y", shear.y, 2 );
    check( "GpiQueryCharDirection", GpiQueryCharDirection( hps ), CHDIRN_TOPBOTTOM );
    GpiQueryTextAlignment( hps, &h, &v );
    check( "GpiQueryTextAlignment horizontal", h, TA_CENTER );
    check( "GpiQueryTextAlignment vertical", v, TA_HALF );
    GpiQueryCharExtra( hps, &fx );
    check( "GpiQueryCharExtra", fx, MAKEFIXED( 3, 0 ) );
    GpiQueryCharBreakExtra( hps, &fx );
    check( "GpiQueryCharBreakExtra", fx, MAKEFIXED( -2, 0 ) );

    /* The single calls; GpiQueryAttrs reads them back, none default. */
    GpiSetCharMode( hps, CM_MODE2 );
    angle.x = 0; angle.y = 1;
    GpiSetCharAngle( hps, &angle );
    GpiSetCharDirection( hps, CHDIRN_RIGHTLEFT );
    GpiSetTextAlignment( hps, TA_RIGHT, TA_TOP );
    GpiSetCharExtra( hps, MAKEFIXED( 1, 0 ) );
    memset( &cb, 0, sizeof( cb ) );
    check( "GpiQueryAttrs after the single calls: no default",
           GpiQueryAttrs( hps, PRIM_CHAR, TEXT_BITS, &cb ), 0 );
    check( "  usPrecision", cb.usPrecision, CM_MODE2 );
    check( "  angle y", cb.ptlAngle.y, 1 );
    check( "  shear y", cb.ptlShear.y, 2 );
    check( "  direction", cb.usDirection, CHDIRN_RIGHTLEFT );
    check( "  alignment", cb.usTextAlign, TA_RIGHT | TA_TOP );
    check( "  extra", cb.fxExtra, MAKEFIXED( 1, 0 ) );
    check( "  break extra", cb.fxBreakExtra, MAKEFIXED( -2, 0 ) );

    /* Invalid values fail and change nothing. */
    cb.ptlShear.x = 1; cb.ptlShear.y = 0;
    check( "a horizontal shear fails", GpiSetAttrs( hps, PRIM_CHAR, CBB_SHEAR | CBB_EXTRA, 0, &cb ), FALSE );
    check( "  PMERR_INV_CHAR_SHEAR_ATTR", ERRORIDERROR( WinGetLastError( 0 ) ), PMERR_INV_CHAR_SHEAR_ATTR );
    cb.usDirection = 9;
    check( "an unknown direction fails", GpiSetAttrs( hps, PRIM_CHAR, CBB_DIRECTION, 0, &cb ), FALSE );
    cb.usPrecision = 7;
    check( "an unknown mode fails", GpiSetAttrs( hps, PRIM_CHAR, CBB_MODE, 0, &cb ), FALSE );
    cb.usTextAlign = 0x0909;
    check( "an unknown alignment fails", GpiSetAttrs( hps, PRIM_CHAR, CBB_TEXT_ALIGN, 0, &cb ), FALSE );
    check( "  direction unchanged", GpiQueryCharDirection( hps ), CHDIRN_RIGHTLEFT );
    check( "  mode unchanged", GpiQueryCharMode( hps ), CM_MODE2 );

    /* The default mask resets them without reading the bundle. */
    check( "GpiSetAttrs with every default", GpiSetAttrs( hps, PRIM_CHAR, TEXT_BITS, TEXT_BITS, NULL ), TRUE );
    check( "  all default again", GpiQueryAttrs( hps, PRIM_CHAR, TEXT_BITS, &cb ), TEXT_BITS );
    check( "  direction", GpiQueryCharDirection( hps ), CHDIRN_LEFTRIGHT );
}

int main( void )
{
    HAB   hab;
    HMQ   hmq;
    HWND  frame, client;
    HPS   hps;
    ULONG fl = FCF_TITLEBAR;
    char  line[64], *p;

    hab = WinInitialize( 0 );
    hmq = WinCreateMsgQueue( hab, 0 );
    WinRegisterClass( hab, "CharBundle", WinDefWindowProc, 0, 0 );
    frame = WinCreateStdWindow( HWND_DESKTOP, 0, &fl, "CharBundle", "Character bundle",
                                0, NULLHANDLE, 0, &client );
    hps = WinGetPS( client );
    run( hps );
    WinReleasePS( hps );

    p = append_str( line, "pm_charbundle_test: done " );
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
