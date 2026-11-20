/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * cmd_tool.c - a probe for the built-in CMD.EXE's integration tests
 *
 * The first argument chooses what it does:
 *
 *   exit N    ends with exit code N (0 ... 255)
 *   cat       copies standard input to standard output, unchanged
 *   count     prints "cmd_tool: count <bytes>" for the bytes of standard
 *             input, to its end
 *   cwd       prints "cmd_tool: cwd X:\DIR": the current drive and its
 *             current directory, from DosQueryCurrentDisk and
 *             DosQueryCurrentDir
 *   yes       writes "y" lines to standard output until a write fails (the
 *             reader closed the pipe) or YES_MAX_LINES were written
 *   touch NAME creates the file NAME (DosOpen, replacing it) holding
 *             "cmd_tool: touched\r\n"; it writes nothing to standard
 *             output, so it can run detached
 *   exec DIR  sets the current directory to DIR (DosSetCurrentDir), then
 *             runs itself in `cwd` mode with DosExecPgm (EXEC_SYNC): the
 *             child prints the directory it started in. The child mode
 *             starts nothing, so the program never starts itself twice.
 *
 * Anything else prints "cmd_tool: usage" and ends with exit code 2. The
 * program uses DosRead and DosWrite on handles 0 and 1 only, so it reads
 * whatever the shell gives it: a pipe, a file or the terminal.
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define STDIN  0
#define STDOUT 1

static void write_stdout( const char *s, ULONG len )
{
    ULONG written;
    DosWrite( STDOUT, (PVOID)s, len, &written );
}

static void write_str( const char *s )
{
    write_stdout( s, strlen( s ) );
}

static char *append_ulong( char *p, ULONG n )
{
    char tmp[12];
    int  i = 0;
    do { tmp[i++] = (char)( '0' + n % 10 ); n /= 10; } while( n );
    while( i ) *p++ = tmp[--i];
    *p = '\0';
    return p;
}

static ULONG parse_ulong( const char *s )
{
    ULONG n = 0;
    while( *s >= '0' && *s <= '9' ) n = n * 10 + (ULONG)( *s++ - '0' );
    return n;
}

static int do_cat( void )
{
    char  buf[512];
    ULONG n;
    while( DosRead( STDIN, buf, sizeof( buf ), &n ) == NO_ERROR && n > 0 )
        write_stdout( buf, n );
    return 0;
}

static int do_count( void )
{
    char  buf[512];
    char  line[48];
    ULONG n, total = 0;
    while( DosRead( STDIN, buf, sizeof( buf ), &n ) == NO_ERROR && n > 0 )
        total += n;
    strcpy( line, "cmd_tool: count " );
    strcpy( append_ulong( line + strlen( line ), total ), "\r\n" );
    write_str( line );
    return 0;
}

static int do_cwd( void )
{
    ULONG disk, map;
    char  dir[CCHMAXPATH];
    ULONG len = sizeof( dir );
    char  line[CCHMAXPATH + 32];
    if( DosQueryCurrentDisk( &disk, &map ) != NO_ERROR ) return 1;
    if( DosQueryCurrentDir( 0, (PBYTE)dir, &len ) != NO_ERROR ) return 1;
    strcpy( line, "cmd_tool: cwd " );
    line[14] = (char)( 'A' + disk - 1 );
    line[15] = ':';
    line[16] = '\\';
    strcpy( line + 17, dir );
    strcat( line, "\r\n" );
    write_str( line );
    return 0;
}

static int do_exec( const char *dir )
{
    PPIB        ppib;
    PTIB        ptib;
    char        self[CCHMAXPATH];
    char        args[16];
    char        fail[32];
    RESULTCODES res;
    if( DosSetCurrentDir( (PSZ)dir ) != NO_ERROR ) {
        write_str( "cmd_tool: exec DosSetCurrentDir failed\r\n" );
        return 1;
    }
    DosGetInfoBlocks( &ptib, &ppib );
    if( DosQueryModuleName( ppib->pib_hmte, sizeof( self ), self ) != NO_ERROR ) return 1;
    /* "cmd_tool\0cwd\0\0": the program name, then the argument string. */
    memcpy( args, "cmd_tool\0cwd\0\0", 15 );
    if( DosExecPgm( fail, sizeof( fail ), EXEC_SYNC, (PSZ)args, NULL, &res, (PSZ)self ) != NO_ERROR ) {
        write_str( "cmd_tool: exec DosExecPgm failed\r\n" );
        return 1;
    }
    return (int)res.codeResult;
}

/* Enough to fill any pipe many times over; a bound, not a goal. */
#define YES_MAX_LINES 10000000UL

static int do_yes( void )
{
    ULONG i, written;
    for( i = 0; i < YES_MAX_LINES; i++ ) {
        if( DosWrite( STDOUT, "y\r\n", 3, &written ) != NO_ERROR || written != 3 ) return 0;
    }
    return 1;
}

static int do_touch( const char *name )
{
    static const char text[] = "cmd_tool: touched\r\n";
    HFILE h;
    ULONG action, written;
    if( DosOpen( (PSZ)name, &h, &action, 0, FILE_NORMAL,
                 OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_REPLACE_IF_EXISTS,
                 OPEN_ACCESS_WRITEONLY | OPEN_SHARE_DENYNONE, NULL ) != NO_ERROR ) return 1;
    DosWrite( h, (PVOID)text, sizeof( text ) - 1, &written );
    DosClose( h );
    return 0;
}

int main( int argc, char **argv )
{
    const char *mode = argc >= 2 ? argv[1] : "";
    if( strcmp( mode, "exit" ) == 0 && argc >= 3 ) return (int)( parse_ulong( argv[2] ) & 0xFF );
    if( strcmp( mode, "cat" ) == 0 ) return do_cat();
    if( strcmp( mode, "count" ) == 0 ) return do_count();
    if( strcmp( mode, "cwd" ) == 0 ) return do_cwd();
    if( strcmp( mode, "exec" ) == 0 && argc >= 3 ) return do_exec( argv[2] );
    if( strcmp( mode, "touch" ) == 0 && argc >= 3 ) return do_touch( argv[2] );
    if( strcmp( mode, "yes" ) == 0 ) return do_yes();
    write_str( "cmd_tool: usage\r\n" );
    return 2;
}
