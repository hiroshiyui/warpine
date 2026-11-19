/*
 * greeter.c - class Greeter in GREETER.DLL, for Warpine's class-loading
 * check (ADR-0062). The DLL exports SOMInitModule, which somFindClass
 * asks SOMLoadModule to call; the program never imports it.
 */
#include <stdio.h>
#include <som.h>

typedef SOMObject Greeter;
typedef void SOMLINK somTP_Greeter_greet( Greeter *somSelf );

SOMEXTERN struct GreeterClassDataStructure {
    SOMClass *classObject;
    somMToken greet;
} SOMDLINK GreeterClassData;
struct GreeterClassDataStructure SOMDLINK GreeterClassData;

static void SOMLINK greet( Greeter *somSelf )
{
    printf( "Greeter %s says hello\n", _somGetClassName( somSelf ) );
}

SOMClass * SOMLINK GreeterNewClass( integer4 major, integer4 minor )
{
    SOMClass *cls;

    if( GreeterClassData.classObject ) return( GreeterClassData.classObject );
    SOM_NEW_CLASS( cls, "Greeter", SOMObjectClassData.classObject, 0, 1, major, minor );
    GreeterClassData.greet = SOM_ADD_METHOD( cls, "greet", greet );
    _somClassReady( cls );
    GreeterClassData.classObject = cls;
    return( cls );
}

/* The module's initialization function: its classes. */
void SOMLINK SOMInitModule( integer4 major, integer4 minor, zString className )
{
    printf( "SOMInitModule(%ld, %ld, %s)\n", major, minor, className );
    GreeterNewClass( major, minor );
}
