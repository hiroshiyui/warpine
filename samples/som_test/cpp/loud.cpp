/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * loud.cpp - class Loud: a Hello that counts its greetings and shouts.
 * Warpine's own test of the C++ bindings (ADR-0061).
 */
#define Loud_Class_Source
#include "loud.xih"

#include <stdio.h>

SOM_Scope void SOMLINK somInit( Loud *somSelf )
{
    LoudData *somThis = LoudGetData( somSelf );

    Loud_parent_SOMObject_somInit( somSelf );
    _greetings = 0;
    printf( "Loud::somInit\n" );
}

SOM_Scope void SOMLINK somUninit( Loud *somSelf )
{
    LoudData *somThis = LoudGetData( somSelf );

    printf( "Loud::somUninit after %ld greeting(s)\n", _greetings );
    Loud_parent_SOMObject_somUninit( somSelf );
}

SOM_Scope void SOMLINK sayHello( Loud *somSelf, Environment *ev )
{
    LoudData *somThis = LoudGetData( somSelf );

    printf( "LOUD: " );
    Loud_parent_Hello_sayHello( somSelf, ev );
    _greetings++;
}

SOM_Scope long SOMLINK shout( Loud *somSelf, Environment *ev, long times )
{
    LoudData *somThis = LoudGetData( somSelf );

    while( times-- > 0 ) somSelf->sayHello( ev );
    return( _greetings );
}
