/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * somlay.c - IBM's SOM layouts (ADR-0072)
 *
 * A program built with IBM's SOM bindings reads the root classes'
 * ClassData at IBM's offsets (the IDL release order), method tables
 * through IBM's somMethodTab header, and CClassData.parentMtab as a
 * somParentMtabStruct: the class's own method table, then its parents'.
 * This program does the same, through the hand-written som.h (which
 * follows IBM's release order) and the declarations below, and checks:
 *
 *   tokens at IBM's places in SOMObjectClassData and SOMClassMgrClassData;
 *   the method table header of the class manager;
 *   SOMClassMgrCClassData.parentMtab and somParentNumResolve;
 *   SOMClass methods of IBM's release order: somCheckVersion,
 *   somGetVersionNumbers, somFindSMethod, somLookupMethod;
 *   a method Warpine lacks raising SOMERROR_NotImplementedYet (20171, the Ignore severity)
 *   through the program's own SOMError;
 *   somTestCls and somCheckArgs.
 *
 * Each check is written to stdout:
 *
 *   som_layout_test: <name>: ok
 *   som_layout_test: <name>: FAIL got <value> want <value>
 *
 * followed by "som_layout_test: done <passed>/<total>".
 */
#define INCL_DOS
#include <os2.h>
#include <string.h>
#include "som.h"

/* CClassData.parentMtab: the class's own method table, its parents'
 * tables, then more fields (control blocks, counts) not used here. */
typedef struct somMethodTabList {
    somMethodTab            *mtab;
    struct somMethodTabList *next;
} somMethodTabList;
typedef struct {
    somMethodTab     *mtab;
    somMethodTabList *next;
    SOMClass         *classObject;
    void             *somRenewNoInitNoZeroThunk;
    long              instanceSize;
} somParentMtabStruct;
typedef struct {
    somParentMtabStruct *parentMtab;
    somDToken            instanceDataToken;
} somCClassDataStructure;
SOMEXTERN somCClassDataStructure SOMDLINK SOMObjectCClassData;
SOMEXTERN somCClassDataStructure SOMDLINK SOMClassMgrCClassData;

SOMEXTERN somMethodProc * SOMLINK somParentNumResolve( somParentMtabStruct *mtabs, int n, somMToken method );
SOMEXTERN SOMObject     * SOMLINK somTestCls( SOMObject *obj, SOMClass *cls, char *file, int line );
SOMEXTERN void            SOMLINK somCheckArgs( int argc, char *argv[] );

typedef char * SOMLINK name_proc( SOMObject * );
typedef int    SOMLINK check_version_proc( SOMClass *, long, long );
typedef void   SOMLINK version_numbers_proc( SOMClass *, long *, long * );
typedef somMethodProc * SOMLINK find_proc( SOMClass *, somId );
typedef long   SOMLINK lacking_proc( SOMObject *, long, SOMObject * );

static int  n_checks = 0;
static int  n_passed = 0;
static int  last_error = 0;

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
    char *p = append_str( append_str( line, "som_layout_test: " ), what );
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

static void SOMLINK catch_error( int code, zString file, int line )
{
    last_error = code;
}

int main( void )
{
    SOMClassMgr  *mgr = somEnvironmentNew();
    SOMClass     *cls = SOMClassMgrClassData.classObject;
    somMToken    *tokens = (somMToken *)&SOMObjectClassData;
    somMethodTab *mtab = mgr->mtab;
    somParentMtabStruct *pm = SOMClassMgrCClassData.parentMtab;
    name_proc    *name;
    long          major = 0, minor = 0;
    char         *argv[3];
    lacking_proc *lacking;
    char          line[48];

    /* ClassData: IBM's release order. SOMObject's starts somInit,
     * somUninit, somFree, somDefaultVCopyInit, somGetClassName, ... */
    check( "SOMObjectClassData: somGetClassName is the fifth token",
           tokens[1 + 4] == SOMObjectClassData.somGetClassName, 1 );
    name = (name_proc *)somResolve( mgr, tokens[1 + 4] );
    check( "... and resolves to it", strcmp( name( mgr ), "SOMClassMgr" ) == 0, 1 );
    check( "SOMClassMgrClassData: somFindClsInFile comes first",
           ((somMToken *)&SOMClassMgrClassData)[1] == SOMClassMgrClassData.somFindClsInFile, 1 );

    /* The method table header. */
    check( "mtab classObject", mtab->classObject == cls, 1 );
    check( "mtab className", strcmp( mtab->className, "SOMClassMgr" ) == 0, 1 );
    check( "mtab instanceSize", mtab->instanceSize, _somGetInstanceSize( cls ) );
    check( "mtab dataAlignment", mtab->dataAlignment, 4 );
    check( "mtab mtabSize covers the header and entries",
           mtab->mtabSize > (long)( (char *)&mtab->entries[0] - (char *)mtab ), 1 );

    /* CClassData.parentMtab. */
    check( "parentMtab: the class's own table", pm->mtab == mtab, 1 );
    check( "... its class object", pm->classObject == cls, 1 );
    check( "... its instance size", pm->instanceSize, mtab->instanceSize );
    check( "... then SOMObject's table",
           pm->next->mtab == SOMObjectCClassData.parentMtab->mtab, 1 );
    check( "... and no more", pm->next->next == NULL, 1 );
    check( "SOMObject has no parents", SOMObjectCClassData.parentMtab->next == NULL, 1 );
    name = (name_proc *)somParentNumResolve( pm, 1, SOMObjectClassData.somGetClassName );
    check( "somParentNumResolve: parent 1", strcmp( name( mgr ), "SOMClassMgr" ) == 0, 1 );

    /* SOMClass methods of IBM's release order. */
    check( "somCheckVersion 1.4",
           ((check_version_proc *)somResolve( cls, SOMClassClassData.somCheckVersion ))( cls, 1, 4 ), 1 );
    check( "somCheckVersion 2.0",
           ((check_version_proc *)somResolve( cls, SOMClassClassData.somCheckVersion ))( cls, 2, 0 ), 0 );
    ((version_numbers_proc *)somResolve( cls, SOMClassClassData.somGetVersionNumbers ))( cls, &major, &minor );
    check( "somGetVersionNumbers", major * 10 + minor, 15 );
    check( "somFindSMethod",
           ((find_proc *)somResolve( cls, SOMClassClassData.somFindSMethod ))( cls, somIdFromString( "somGetClassName" ) )
           == somResolve( mgr, SOMObjectClassData.somGetClassName ), 1 );
    check( "somLookupMethod",
           ((find_proc *)somResolve( cls, SOMClassClassData.somLookupMethod ))( cls, somIdFromString( "somFindClass" ) )
           == somResolve( mgr, SOMClassMgrClassData.somFindClass ), 1 );

    /* A method Warpine lacks: SOMERROR_NotImplementedYet with the Ignore severity. */
    SOMError = catch_error;
    lacking = (lacking_proc *)somResolve( mgr, SOMObjectClassData.somDispatchD );
    check( "a lacking method returns 0", lacking( mgr, 0, mgr ), 0 );
    check( "... after SOMError 20171", last_error, 20171 );

    /* somTestCls and somCheckArgs. */
    check( "somTestCls gives the object back",
           somTestCls( mgr, SOMObjectClassData.classObject, __FILE__, __LINE__ ) == mgr, 1 );
    argv[0] = "somlay";
    argv[1] = "-somtc";
    argv[2] = "-somw";
    somCheckArgs( 3, argv );
    check( "somCheckArgs: -somtc", SOM_TraceLevel, 2 );
    check( "somCheckArgs: -somw", SOM_WarnLevel, 1 );

    append_str( append_long( append_str( append_long( append_str( line, "som_layout_test: done " ),
                n_passed ), "/" ), n_checks ), "\r\n" );
    write_stdout( line );
    return 0;
}
