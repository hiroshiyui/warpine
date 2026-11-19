/*
 * hello.h - C usage bindings of class Hello (hello.idl of the Open Watcom
 * SOM samples), written by hand for Warpine as the SOM compiler would
 * emit them (ADR-0059).
 */
#ifndef SOM_Hello_h
#define SOM_Hello_h

#include <som.h>

typedef SOMObject Hello;

typedef void SOMLINK somTP_Hello_sayHello( Hello *somSelf, Environment *ev );
typedef somTP_Hello_sayHello *somTD_Hello_sayHello;

SOMEXTERN SOMClass * SOMLINK HelloNewClass( integer4 major, integer4 minor );

SOMEXTERN struct HelloClassDataStructure {
    SOMClass *classObject;
    somMToken sayHello;
} SOMDLINK HelloClassData;

#define _Hello  (HelloClassData.classObject ? HelloClassData.classObject : HelloNewClass( 0, 0 ))
#define HelloNew()  ((Hello *)_somNew( _Hello ))

#define _sayHello( somSelf, ev ) \
    SOM_Resolve( somSelf, Hello, sayHello )( (Hello *)(somSelf), (ev) )

#endif
