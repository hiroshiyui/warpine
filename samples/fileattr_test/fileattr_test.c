/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * fileattr_test.c - setting file attributes
 *
 * DosSetPathInfo / DosSetFileInfo (FIL_STANDARD) and DosSetFileMode apply
 * the attributes of a FILESTATUS3. FILE_READONLY is the one with an effect:
 *
 *   - a read-only file reports FILE_READONLY, cannot be opened for writing
 *     and cannot be deleted (ERROR_ACCESS_DENIED)
 *   - clearing the attribute (by path, by handle, or with DosSetFileMode)
 *     makes it writable and deletable again
 *   - a read-only directory still takes new files
 *
 * DosSetFileMode (DOSCALLS 84, the 16-bit DOS16SETFILEMODE) is called
 * directly with a 32-bit name pointer, as Warpine accepts it.
 *
 * Output: "fileattr_test: <name>: ok" or "... FAIL got <v> want <v>", then
 * "fileattr_test: done <passed>/<total>"; the exit code is the number of
 * failed checks.
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <string.h>

#define TEST_FILE "C:\\FA_TEST.TMP"
#define TEST_DIR  "C:\\FA_DIR"
#define DIR_FILE  "C:\\FA_DIR\\INSIDE.TMP"

#ifdef __WATCOMC__
extern USHORT APIENTRY DOS16SETFILEMODE( PSZ name, USHORT attr, ULONG reserved );
#else
#define DOS16SETFILEMODE DosSetFileMode  /* the SDK's name for DOSCALLS.84 */
#endif

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
    char *p = append_str( append_str( line, "fileattr_test: " ), what );
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

static ULONG attrs_of( const char *name )
{
    FILESTATUS3 fs;
    if( DosQueryPathInfo( (PSZ)name, FIL_STANDARD, &fs, sizeof( fs ) ) != NO_ERROR ) return 0xFFFF;
    return fs.attrFile;
}

/* Set only the attributes by path: times 0 / 0 leave the times alone. */
static APIRET set_attrs( const char *name, ULONG attrs )
{
    FILESTATUS3 fs;
    memset( &fs, 0, sizeof( fs ) );
    fs.attrFile = attrs;
    return DosSetPathInfo( (PSZ)name, FIL_STANDARD, &fs, sizeof( fs ), 0 );
}

static APIRET open_file( const char *name, ULONG mode, HFILE *phf )
{
    ULONG action;
    return DosOpen( (PSZ)name, phf, &action, 0, FILE_NORMAL,
                    OPEN_ACTION_CREATE_IF_NEW | OPEN_ACTION_OPEN_IF_EXISTS,
                    mode | OPEN_SHARE_DENYNONE, NULL );
}

int main( void )
{
    HFILE       hf;
    FILESTATUS3 fs;
    char        line[48];

    DOS16SETFILEMODE( (PSZ)TEST_FILE, FILE_NORMAL, 0 );
    DosDelete( (PSZ)TEST_FILE );
    check( "create", open_file( TEST_FILE, OPEN_ACCESS_READWRITE, &hf ), NO_ERROR );
    DosClose( hf );
    check( "new file is not read-only", attrs_of( TEST_FILE ) & FILE_READONLY, 0 );

    /* By path. */
    check( "DosSetPathInfo FILE_READONLY", set_attrs( TEST_FILE, FILE_READONLY ), NO_ERROR );
    check( "reports FILE_READONLY", attrs_of( TEST_FILE ) & FILE_READONLY, FILE_READONLY );
    check( "no opening for writing", open_file( TEST_FILE, OPEN_ACCESS_WRITEONLY, &hf ), ERROR_ACCESS_DENIED );
    check( "no deleting", DosDelete( (PSZ)TEST_FILE ), ERROR_ACCESS_DENIED );

    /* With DosSetFileMode. */
    check( "DosSetFileMode FILE_NORMAL", DOS16SETFILEMODE( (PSZ)TEST_FILE, FILE_NORMAL, 0 ), NO_ERROR );
    check( "writable again", attrs_of( TEST_FILE ) & FILE_READONLY, 0 );
    check( "DosSetFileMode FILE_READONLY", DOS16SETFILEMODE( (PSZ)TEST_FILE, FILE_READONLY, 0 ), NO_ERROR );
    check( "read-only again", attrs_of( TEST_FILE ) & FILE_READONLY, FILE_READONLY );

    /* By handle: a read-only file opened for reading. */
    check( "open for reading", open_file( TEST_FILE, OPEN_ACCESS_READONLY, &hf ), NO_ERROR );
    DosQueryFileInfo( hf, FIL_STANDARD, &fs, sizeof( fs ) );
    fs.attrFile &= ~FILE_READONLY;
    memset( &fs.fdateCreation, 0, 3 * ( sizeof( FDATE ) + sizeof( FTIME ) ) );
    check( "DosSetFileInfo clears FILE_READONLY", DosSetFileInfo( hf, FIL_STANDARD, &fs, sizeof( fs ) ), NO_ERROR );
    DosClose( hf );
    check( "cleared by handle", attrs_of( TEST_FILE ) & FILE_READONLY, 0 );
    check( "opening for writing works", open_file( TEST_FILE, OPEN_ACCESS_WRITEONLY, &hf ), NO_ERROR );
    DosClose( hf );
    check( "deleting works", DosDelete( (PSZ)TEST_FILE ), NO_ERROR );

    /* A read-only directory still takes files. */
    DosDelete( (PSZ)DIR_FILE );
    DosDeleteDir( (PSZ)TEST_DIR );
    check( "create directory", DosCreateDir( (PSZ)TEST_DIR, NULL ), NO_ERROR );
    check( "directory FILE_READONLY", set_attrs( TEST_DIR, FILE_DIRECTORY | FILE_READONLY ), NO_ERROR );
    check( "file in a read-only directory", open_file( DIR_FILE, OPEN_ACCESS_READWRITE, &hf ), NO_ERROR );
    DosClose( hf );
    DosDelete( (PSZ)DIR_FILE );
    check( "remove directory", DosDeleteDir( (PSZ)TEST_DIR ), NO_ERROR );

    append_str( append_long( append_str( append_long( append_str( line, "fileattr_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    return n_checks - n_passed;
}
