/*
 * argv_test.c - the C runtime's argc / argv / environment and process ids,
 * in a program started with arguments and in its DosExecPgm children
 *
 * Run it with arguments ("argv_test alpha beta gamma"). It writes its argc,
 * argv and process ids, then compares argv with the command line in its
 * process block (pib_pchcmd) and starts copies of itself:
 *
 *   - "child <ppid> one two three", EXEC_ASYNC with its own environment
 *     (ARGV_TEST_VAR=hello); the child checks its argv, its environment and
 *     that its parent id (pib_ulppid) is the <ppid> it was given, and ends
 *     with its own process id as exit code (the low 8 bits, all a host exit
 *     status carries), which the parent compares with the id DosExecPgm
 *     returned and DosWaitChild reported;
 *   - "childsync <ppid> x", EXEC_SYNC; it starts "grandchild <its pid>"
 *     (EXEC_SYNC) and checks that one's id the same way, and ends with its
 *     own id too, which must differ from every other id seen;
 *   - "sleeper", EXEC_ASYNC, ended with DosKillProcess.
 *
 * Every check writes one line, "argv_test: ok <what>" or
 * "argv_test: FAIL <what>". A copy recognises its mode from pib_pchcmd, not
 * from argv, so a broken argv cannot make it start copies without end.
 */
#define INCL_DOS
#define INCL_DOSERRORS
#include <os2.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int failures = 0;

/* The part of a child's exit code (its pid) that reaches the parent. */
#define EXIT_MASK 0xFFu

static void check( int cond, const char *what )
{
    printf( "argv_test: %s %s\n", cond ? "ok" : "FAIL", what );
    fflush( stdout );
    if( !cond ) failures++;
}

/* The argument string of the process block: what follows the program name. */
static const char *pib_args( PPIB ppib )
{
    const char *p = ppib->pib_pchcmd;
    if( p == NULL ) return "";
    return p + strlen( p ) + 1;
}

/* Whether argv[1..argc-1] are the blank-separated words of `args`. */
static int argv_matches( int argc, char **argv, const char *args )
{
    char  copy[512];
    char *tok;
    int   i = 1;

    strncpy( copy, args, sizeof( copy ) - 1 );
    copy[sizeof( copy ) - 1] = '\0';
    for( tok = strtok( copy, " \t" ); tok != NULL; tok = strtok( NULL, " \t" ) ) {
        if( i >= argc || strcmp( argv[i], tok ) != 0 ) return 0;
        i++;
    }
    return i == argc;
}

static void show( const char *who, int argc, char **argv, PPIB ppib )
{
    int i;
    printf( "argv_test: %s pid=%lu ppid=%lu argc=%d\n", who,
            ppib->pib_ulpid, ppib->pib_ulppid, argc );
    for( i = 0; i < argc; i++ ) {
        printf( "argv_test: %s argv[%d]=%s\n", who, i, argv[i] ? argv[i] : "(null)" );
    }
    fflush( stdout );
}

/* The first word of `args` (at most n-1 characters). */
static void first_word( const char *args, char *out, int n )
{
    int i = 0;
    while( *args == ' ' ) args++;
    while( args[i] != '\0' && args[i] != ' ' && i < n - 1 ) { out[i] = args[i]; i++; }
    out[i] = '\0';
}

/* The second word of `args` as a number (0 if none). */
static ULONG second_number( const char *args )
{
    while( *args == ' ' ) args++;
    while( *args != '\0' && *args != ' ' ) args++;
    return strtoul( args, NULL, 10 );
}

static APIRET start_copy( const char *path, ULONG flag, const char *args, PSZ env, RESULTCODES *res );

/* `mode` is "child" (asynchronous, own environment), "childsync" (which
   starts a grandchild) or "grandchild". */
static int child_main( int argc, char **argv, PPIB ppib, const char *mode )
{
    int         sync = strcmp( mode, "child" ) != 0;
    char        path[CCHMAXPATH], gargs[64], line[128];
    RESULTCODES res;
    const char *args = pib_args( ppib );
    const char *var;
    ULONG       want_ppid = second_number( args );

    show( mode, argc, argv, ppib );
    check( argv_matches( argc, argv, args ), "child argv matches its command line" );
    check( argc >= 2 && strcmp( argv[1], mode ) == 0, "child argv[1] is its mode" );
    check( ppib->pib_ulppid == want_ppid, "child pib_ulppid is the parent's pid" );
    check( ppib->pib_ulpid != want_ppid && ppib->pib_ulpid != 0, "child pid differs from its parent's" );
    if( !sync ) {
        var = getenv( "ARGV_TEST_VAR" );
        check( var != NULL && strcmp( var, "hello" ) == 0, "child environment from DosExecPgm" );
        check( getenv( "PATH" ) == NULL, "child environment is only the one given" );
    }
    if( strcmp( mode, "childsync" ) == 0
        && DosQueryModuleName( ppib->pib_hmte, sizeof( path ), path ) == NO_ERROR ) {
        sprintf( gargs, "grandchild %lu", ppib->pib_ulpid );
        check( start_copy( path, EXEC_SYNC, gargs, NULL, &res ) == NO_ERROR,
               "DosExecPgm(EXEC_SYNC) grandchild" );
        sprintf( line, "grandchild's pid is new (%lu)", res.codeResult );
        check( res.codeResult != 1 && res.codeResult != ( ppib->pib_ulpid & EXIT_MASK )
               && res.codeResult != ( want_ppid & EXIT_MASK ), line );
    }
    /* Exit codes reach the parent through a host process: low 8 bits only. */
    return failures ? 1 : (int)( ppib->pib_ulpid & EXIT_MASK );
}

/* Run a copy of this program with `args` after the program name. */
static APIRET start_copy( const char *path, ULONG flag, const char *args, PSZ env, RESULTCODES *res )
{
    char block[CCHMAXPATH + 128];
    char fail[64];
    size_t n = strlen( path );

    memset( block, 0, sizeof( block ) );
    strcpy( block, path );
    strcpy( block + n + 1, args );
    return DosExecPgm( fail, sizeof( fail ), flag, block, env, res, (PSZ)path );
}

int main( int argc, char **argv )
{
    PTIB        ptib;
    PPIB        ppib;
    char        path[CCHMAXPATH], mode[16], args[64], line[128];
    RESULTCODES res, waited;
    PID         async_pid, got_pid, sleeper;
    ULONG       me;
    APIRET      rc;

    DosGetInfoBlocks( &ptib, &ppib );
    first_word( pib_args( ppib ), mode, sizeof( mode ) );
    if( strcmp( mode, "child" ) == 0 || strcmp( mode, "childsync" ) == 0
        || strcmp( mode, "grandchild" ) == 0 ) {
        return child_main( argc, argv, ppib, mode );
    }
    if( strcmp( mode, "sleeper" ) == 0 ) { DosSleep( 20000 ); return 0; }

    me = ppib->pib_ulpid;
    show( "parent", argc, argv, ppib );
    check( argc >= 2, "parent argv has the arguments" );
    check( argv_matches( argc, argv, pib_args( ppib ) ), "parent argv matches its command line" );
    check( argv[0] != NULL && strlen( argv[0] ) > 0, "parent argv[0] is the program name" );
    check( getenv( "PATH" ) != NULL, "parent environment has PATH" );
    check( me != 0, "parent pid is set" );

    if( DosQueryModuleName( ppib->pib_hmte, sizeof( path ), path ) != NO_ERROR ) {
        check( 0, "DosQueryModuleName" );
        return 1;
    }

    /* Asynchronous child with its own environment. */
    sprintf( args, "child %lu one two three", me );
    rc = start_copy( path, EXEC_ASYNC, args, "ARGV_TEST_VAR=hello\0", &res );
    check( rc == NO_ERROR, "DosExecPgm(EXEC_ASYNC) child" );
    async_pid = res.codeTerminate;
    check( async_pid != 0 && async_pid != me, "DosExecPgm returns a new pid" );
    rc = DosWaitChild( DCWA_PROCESS, DCWW_WAIT, &waited, &got_pid, async_pid );
    check( rc == NO_ERROR && got_pid == async_pid, "DosWaitChild reports that pid" );
    sprintf( line, "child's own pid is the one DosExecPgm returned (%lu, exit %lu)",
             async_pid, waited.codeResult );
    check( waited.codeResult == ( async_pid & EXIT_MASK ), line );

    /* Synchronous child. */
    sprintf( args, "childsync %lu x", me );
    rc = start_copy( path, EXEC_SYNC, args, NULL, &res );
    check( rc == NO_ERROR, "DosExecPgm(EXEC_SYNC) child" );
    sprintf( line, "sync child's pid is new (%lu)", res.codeResult );
    check( res.codeResult != 1 && res.codeResult != ( me & EXIT_MASK )
           && res.codeResult != ( async_pid & EXIT_MASK ), line );

    /* A child ended by DosKillProcess under the pid DosExecPgm gave. */
    rc = start_copy( path, EXEC_ASYNC, "sleeper", NULL, &res );
    sleeper = res.codeTerminate;
    check( rc == NO_ERROR && sleeper != async_pid && sleeper != me, "sleeper started with a new pid" );
    check( DosKillProcess( DKP_PROCESS, sleeper ) == NO_ERROR, "DosKillProcess(sleeper pid)" );

    printf( "argv_test: done, %d failed\n", failures );
    return failures ? 1 : 0;
}
