/*
 * findcls.c - Warpine's check of SOM class loading and the replaceable
 * runtime variables (ADR-0062):
 *   - somFindClass loads a class's DLL by the class's name and calls its
 *     SOMInitModule; a second call finds the class without loading;
 *   - somFindClsInFile through the program's own SOMLoadModule, which
 *     calls the default one; the DLL has only <Class>NewClass;
 *   - a class without a file is not found; somLocateClassFile;
 *   - SOMDeleteModule, somSetOutChar, a program's SOMError, and the
 *     default SOMError ending the program on a fatal code.
 * The classes are used by name only (somResolveByName): the program
 * imports nothing from their DLLs.
 */
#include <stdio.h>
#include <ctype.h>
#include <som.h>

typedef void SOMLINK greet_fn( SOMObject * );
typedef void SOMLINK echo_fn( SOMObject *, zString );

static somTD_SOMLoadModule *default_load;
static void *loaded;

static int SOMLINK my_load( zString cls, zString file, zString func, long major, long minor, void **handle )
{
    int rc;

    printf( "SOMLoadModule(%s, %s, %s, %ld, %ld)\n", cls, file, func, major, minor );
    rc = default_load( cls, file, func, major, minor, handle );
    printf( "  -> %d, %s\n", rc, *handle ? "a module" : "no module" );
    loaded = *handle;
    return( rc );
}

static int SOMLINK upper_out( char c )
{
    putchar( toupper( (unsigned char)c ) );
    return( 1 );
}

static void SOMLINK my_error( int code, zString file, int line )
{
    printf( "my SOMError: code %d, %s\n", code, file && line > 0 ? "with a place" : "no place" );
}

int main( void )
{
    SOMClassMgr *mgr;
    SOMClass    *cls;
    SOMObject   *obj;
    somTD_SOMError *default_error;

    setvbuf( stdout, NULL, _IONBF, 0 );     /* in order with somPrintf */
    mgr = somEnvironmentNew();

    cls = _somFindClass( mgr, somIdFromString( "Greeter" ), 1, 2 );
    printf( "found %s\n", cls ? _somGetName( cls ) : "nothing" );
    obj = _somNew( cls );
    ((greet_fn *)somResolveByName( obj, "greet" ))( obj );
    _somFree( obj );
    printf( "again: %s\n",
            _somFindClass( mgr, somIdFromString( "Greeter" ), 1, 2 ) == cls ? "the same class" : "another" );

    default_load = SOMLoadModule;
    SOMLoadModule = my_load;
    cls = _somFindClsInFile( mgr, somIdFromString( "Echo" ), 3, 4, "echoes" );
    SOMLoadModule = default_load;
    printf( "found %s\n", cls ? _somGetName( cls ) : "nothing" );
    obj = _somNew( cls );
    ((echo_fn *)somResolveByName( obj, "echo" ))( obj, "echo" );
    _somFree( obj );

    printf( "Nowhere: %s, its file would be %s\n",
            _somFindClass( mgr, somIdFromString( "Nowhere" ), 0, 0 ) ? "found" : "not found",
            _somLocateClassFile( mgr, somIdFromString( "Nowhere" ), 0, 0 ) );
    printf( "SOMDeleteModule -> %d\n", SOMDeleteModule( loaded ) );

    somSetOutChar( upper_out );
    somPrintf( "quiet %d\n", 7 );
    somSetOutChar( 0 );
    somPrintf( "quiet %d\n", 8 );

    default_error = SOMError;
    SOMError = my_error;
    SOM_Error( 20012 );
    SOMError = default_error;
    printf( "a fatal error next\n" );
    SOM_Error( 49 );
    printf( "not reached\n" );
    return( 0 );
}
