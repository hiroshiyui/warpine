/*
 * echo.c - class Echo in ECHOES.DLL, for Warpine's class-loading check
 * (ADR-0062). No SOMInitModule: the loader calls EchoNewClass.
 */
#include <stdio.h>
#include <som.h>

typedef SOMObject Echo;

SOMEXTERN struct EchoClassDataStructure {
    SOMClass *classObject;
    somMToken echo;
} SOMDLINK EchoClassData;
struct EchoClassDataStructure SOMDLINK EchoClassData;

static void SOMLINK echo( Echo *somSelf, zString text )
{
    printf( "%s %s\n", text, text );
}

SOMClass * SOMLINK EchoNewClass( integer4 major, integer4 minor )
{
    SOMClass *cls;

    if( EchoClassData.classObject ) return( EchoClassData.classObject );
    printf( "EchoNewClass(%ld, %ld)\n", major, minor );
    SOM_NEW_CLASS( cls, "Echo", SOMObjectClassData.classObject, 0, 1, major, minor );
    EchoClassData.echo = SOM_ADD_METHOD( cls, "echo", echo );
    _somClassReady( cls );
    EchoClassData.classObject = cls;
    return( cls );
}
