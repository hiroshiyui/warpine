/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * sompp.cpp - Warpine's own check of the SOM C++ bindings (ADR-0061):
 * new and delete through the SOM runtime, constructors that initialize
 * once, overrides reached through a base-class pointer, parent calls,
 * instance data, and stack objects that are not SOM objects.
 */
#include <stdio.h>
#include <loud.xh>

int main( void )
{
    Environment *ev = somGetGlobalEnvironment();
    Hello *h;
    Loud  *l;

    h = new Loud;                   /* somNewNoInit, then somDefaultInit once */
    h->sayHello( ev );              /* Loud's override, through a Hello */
    printf( "class %s, isA Hello %d, instance of Hello %d, of Loud %d\n",
            h->somGetClassName(), h->somIsA( _Hello ),
            h->somIsInstanceOf( _Hello ), h->somIsInstanceOf( _Loud ) );
    delete h;                       /* somDestruct( 1 ): Loud's somUninit */
    printf( "deleted: %d\n", somIsObj( h ) );

    l = new Loud;
    printf( "shout -> %ld\n", l->shout( ev, 2 ) );
    l->somFree();

    h = new Hello;
    h->sayHello( ev );
    printf( "a Hello is a Loud: %d\n", h->somIsA( _Loud ) );
    delete h;

    {
        Loud local;                 /* not made by the runtime */
        printf( "stack object is a SOM object: %d\n", somIsObj( &local ) );
    }
    printf( "done\n" );
    return( 0 );
}
