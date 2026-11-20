/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * nest_test.c - a program that starts itself without end, to check
 * Warpine's process nesting limit
 *
 * Each copy runs itself again with DosExecPgm(EXEC_SYNC) and waits for it.
 * Warpine runs every OS/2 process as a host process of its own and bounds
 * how deeply they nest, so at some depth DosExecPgm fails (on OS/2 the
 * system would run out of process slots or memory instead). That copy
 * writes
 *
 *   nest_test: DosExecPgm refused rc=<rc>
 *
 * and ends with code 0; every copy above it writes
 *
 *   nest_test: child ended with <code>
 *
 * and ends with its child's code + 1, so the first copy's exit code is the
 * number of copies it started below it.
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

static char *append_str( char *p, const char *s )
{
    while( *s ) *p++ = *s++;
    *p = '\0';
    return p;
}

static char *append_long( char *p, ULONG u )
{
    char tmp[12];
    int  i = 0;
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

int main( void )
{
    PTIB        ptib;
    PPIB        ppib;
    char        path[CCHMAXPATH], args[CCHMAXPATH + 2], fail[64], line[64], *p;
    RESULTCODES res;
    APIRET      rc;

    DosGetInfoBlocks( &ptib, &ppib );
    if( DosQueryModuleName( ppib->pib_hmte, sizeof( path ), path ) != NO_ERROR ) return 255;
    /* Argument block: the program name, NUL, no arguments, NUL. */
    memset( args, 0, sizeof( args ) );
    strcpy( args, path );
    rc = DosExecPgm( fail, sizeof( fail ), EXEC_SYNC, args, NULL, &res, path );
    p = append_str( line, "nest_test: " );
    if( rc != NO_ERROR ) {
        p = append_str( p, "DosExecPgm refused rc=" );
        p = append_long( p, rc );
        append_str( p, "\r\n" );
        write_stdout( line );
        return 0;
    }
    p = append_str( p, "child ended with " );
    p = append_long( p, res.codeResult );
    append_str( p, "\r\n" );
    write_stdout( line );
    return (int)res.codeResult + 1;
}
