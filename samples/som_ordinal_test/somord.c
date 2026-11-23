/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * somord.c - the SOM kernel imported by ordinal (ADR-0070)
 *
 * Programs linked against IBM's som.lib import SOM.DLL by number. This one
 * does the same (somord.lnk): ordinals 1-47 are IBM's SOM 1.0 exports as
 * osFree lists them. It checks that each import reaches the right entry:
 * the root classes' ClassData and CClassData, the class manager, the
 * <Class>NewClass procedures, SOMClassInitFuncName, SOMMalloc / SOMFree,
 * the id registry, method resolution through a parent's method table, and
 * the printing calls, whose output it catches with its own
 * SOMOutCharRoutine. Each check is written to stdout:
 *
 *   som_ordinal_test: <name>: ok
 *   som_ordinal_test: <name>: FAIL got <value> want <value>
 *
 * followed by "som_ordinal_test: done <passed>/<total>".
 */
#define INCL_DOS
#include <os2.h>
#include <string.h>
#include <stdarg.h>
#include "som.h"

/* SOM 1.0 entries som.h does not declare (SOM Programming Reference). */
typedef struct somMethodTabList {
    somMethodTab            *mtab;
    struct somMethodTabList *next;
} somMethodTabList;
/* parentMtab: the class's own method table, then its parents' (ADR-0072). */
typedef struct {
    somMethodTab     *mtab;
    somMethodTabList *next;
    SOMClass         *classObject;
} somParentMtab;
typedef struct {
    somParentMtab *parentMtab;
    somDToken      instanceDataToken;
} somCClassData;
SOMEXTERN somCClassData SOMDLINK SOMObjectCClassData;
SOMEXTERN somCClassData SOMDLINK SOMClassCClassData;
SOMEXTERN somCClassData SOMDLINK SOMClassMgrCClassData;
SOMEXTERN char * (SOMLINK * SOMDLINK SOMClassInitFuncName)( void );
SOMEXTERN SOMClass      * SOMLINK SOMObjectNewClass( long major, long minor );
SOMEXTERN SOMClass      * SOMLINK SOMClassNewClass( long major, long minor );
SOMEXTERN SOMClass      * SOMLINK SOMClassMgrNewClass( long major, long minor );
SOMEXTERN somMethodProc * SOMLINK somParentResolve( somParentMtab *parents, somMToken method );
SOMEXTERN somId           SOMLINK somCheckId( somId id );
SOMEXTERN int             SOMLINK somRegisterId( somId id );
SOMEXTERN unsigned long   SOMLINK somUniqueKey( somId id );
SOMEXTERN unsigned long   SOMLINK somTotalRegIds( void );
SOMEXTERN void            SOMLINK somSetExpectedIds( unsigned long n );
SOMEXTERN void            SOMLINK somBeginPersistentIds( void );
SOMEXTERN void            SOMLINK somEndPersistentIds( void );
SOMEXTERN void            SOMLINK somPrefixLevel( long level );
SOMEXTERN long            SOMLINK somLPrintf( long level, char *fmt, ... );
SOMEXTERN long            SOMLINK somVprintf( char *fmt, va_list ap );

typedef char * SOMLINK name_proc( SOMObject * );
typedef long   SOMLINK count_proc( SOMClass * );
typedef void   SOMLINK destruct_proc( SOMObject *, unsigned char, void * );

/* What the class Tidy's own somDestruct saw. */
static long destruct_calls = 0;
static long destruct_free  = -1;
static void *destruct_ctrl = (void *)1;

/* Tidy's somDestruct: note the call, then SOMObject's, which frees the
 * object when doFree is set. */
static void SOMLINK tidy_destruct( SOMObject *self, unsigned char doFree, void *ctrl )
{
    destruct_calls++;
    destruct_free = doFree;
    destruct_ctrl = ctrl;
    ((destruct_proc *)somClassResolve( SOMObjectClassData.classObject, SOMObjectClassData.somDestruct ))
        ( self, doFree, ctrl );
}

static int  n_checks = 0;
static int  n_passed = 0;
static char caught[128];
static int  n_caught = 0;

static char *append_str( char *p, const char *s )
{
    while( *s ) *p++ = *s++;
    *p = '\0';
    return p;
}

static char *append_long( char *p, long n )
{
    char          tmp[12];
    int           i = 0;
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
    char *p = append_str( append_str( line, "som_ordinal_test: " ), what );
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

static int SOMLINK catch_char( char c )
{
    if( n_caught < (int)sizeof( caught ) - 1 ) caught[n_caught++] = c;
    caught[n_caught] = '\0';
    return 1;
}

static void caught_is( const char *what, const char *want )
{
    check( what, strcmp( caught, want ) == 0, 1 );
    n_caught = 0;
    caught[0] = '\0';
}

static long vprint( char *fmt, ... )
{
    va_list ap;
    long    n;
    va_start( ap, fmt );
    n = somVprintf( fmt, ap );
    va_end( ap );
    return n;
}

int main( void )
{
    SOMClassMgr   *mgr = somEnvironmentNew();
    somId          hello, again, mine;
    SOMClass      *tidy;
    SOMObject     *obj;
    count_proc    *count;
    char          *fresh = "Fresh";
    unsigned long  before;
    name_proc     *name;
    void          *block;
    char           line[48];

    /* The class manager and the root classes. */
    check( "somEnvironmentNew is SOMClassMgrObject", mgr == SOMClassMgrObject, 1 );
    check( "SOMObjectNewClass", SOMObjectNewClass( 1, 0 ) == SOMObjectClassData.classObject, 1 );
    check( "SOMClassNewClass", SOMClassNewClass( 1, 0 ) == SOMClassClassData.classObject, 1 );
    check( "SOMClassMgrNewClass", SOMClassMgrNewClass( 0, 0 ) == SOMClassMgrClassData.classObject, 1 );
    check( "the manager is a SOMClassMgr", mgr->mtab->classObject == SOMClassMgrClassData.classObject, 1 );

    /* Method resolution, directly and through the parent's table. */
    name = (name_proc *)somResolve( mgr, SOMObjectClassData.somGetClassName );
    check( "somResolve: somGetClassName", strcmp( name( mgr ), "SOMClassMgr" ) == 0, 1 );
    check( "SOMObject has no parent tables", SOMObjectCClassData.parentMtab->next == NULL, 1 );
    check( "SOMClassMgr's parentMtab starts with its own table",
           SOMClassMgrCClassData.parentMtab->mtab == mgr->mtab, 1 );
    check( "... then SOMObject's",
           SOMClassMgrCClassData.parentMtab->next->mtab->classObject == SOMObjectClassData.classObject, 1 );
    name = (name_proc *)somParentResolve( SOMClassMgrCClassData.parentMtab, SOMObjectClassData.somGetClassName );
    check( "somParentResolve: SOMObject's somGetClassName", strcmp( name( mgr ), "SOMClassMgr" ) == 0, 1 );

    /* The runtime variables. */
    check( "SOMClassInitFuncName", strcmp( SOMClassInitFuncName(), "SOMInitModule" ) == 0, 1 );
    block = SOMMalloc( 32 );
    check( "SOMMalloc", block != NULL, 1 );
    SOMFree( block );
    check( "SOM_TraceLevel", SOM_TraceLevel, 0 );

    /* Ids. */
    hello = somIdFromString( "Hello" );
    again = somIdFromString( "Hello" );
    check( "somIdFromString gives each caller its own id", hello != again, 1 );
    check( "somStringFromId", strcmp( somStringFromId( hello ), "Hello" ) == 0, 1 );
    check( "somCompareIds", somCompareIds( hello, again ), 1 );
    /* The caller owns its id and frees it (SOM Programming Reference). */
    SOMFree( hello );
    hello = somIdFromString( "Hello" );
    check( "the string stays registered after SOMFree",
           strcmp( somStringFromId( hello ), "Hello" ) == 0 && somUniqueKey( hello ) == somUniqueKey( again ), 1 );
    somSetExpectedIds( 100 );
    somBeginPersistentIds();
    mine = &fresh;
    before = somTotalRegIds();
    check( "somRegisterId: new", somRegisterId( mine ), 1 );
    check( "somRegisterId: again", somRegisterId( mine ), 0 );
    somEndPersistentIds();
    check( "somTotalRegIds counts it", somTotalRegIds() - before, 1 );
    check( "somCheckId gives the id back", somCheckId( mine ) == mine, 1 );
    check( "somUniqueKey: one key per string",
           somUniqueKey( mine ) == somUniqueKey( somIdFromString( "Fresh" ) ), 1 );

    /* somGetNumStaticMethods counts inherited methods too. */
    count = (count_proc *)somResolve( SOMClassClassData.classObject, SOMClassClassData.somGetNumStaticMethods );
    check( "somGetNumStaticMethods includes inherited ones",
           count( SOMClassClassData.classObject ) > count( SOMObjectClassData.classObject )
           && count( SOMObjectClassData.classObject ) > 0, 1 );
    check( "... as many as somGetNumMethods",
           count( SOMClassClassData.classObject )
           == ((count_proc *)somResolve( SOMClassClassData.classObject, SOMClassClassData.somGetNumMethods ))
                  ( SOMClassClassData.classObject ), 1 );

    /* somFree calls the class's own somDestruct (doFree set, no control). */
    tidy = _somNew( SOMClassClassData.classObject );
    _somInitClass( tidy, "Tidy", SOMObjectClassData.classObject, 0, 0, 1, 0 );
    _somOverrideSMethod( tidy, somIdFromString( "somDestruct" ), (somMethodProc *)tidy_destruct );
    _somClassReady( tidy );
    obj = _somNew( tidy );
    _somFree( obj );
    check( "somFree calls the class's somDestruct", destruct_calls, 1 );
    check( "... with doFree set", destruct_free, 1 );
    check( "... and a null control", destruct_ctrl == NULL, 1 );
    check( "... which freed the object", somIsObj( obj ), 0 );

    /* Printing through the program's own SOMOutCharRoutine. */
    SOMOutCharRoutine = catch_char;
    check( "somPrintf's count", somPrintf( "n=%d", 7 ), 3 );
    caught_is( "somPrintf's text", "n=7" );
    somPrefixLevel( 2 );
    caught_is( "somPrefixLevel", "    " );
    somLPrintf( 1, "%s!", "hi" );
    caught_is( "somLPrintf", "  hi!" );
    check( "somVprintf's count", vprint( "%d-%s", 42, "x" ), 4 );
    caught_is( "somVprintf's text", "42-x" );
    somSetOutChar( 0 );

    append_str( append_long( append_str( append_long( append_str( line, "som_ordinal_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    return 0;
}
