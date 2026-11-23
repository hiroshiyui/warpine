/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * main.c - som_idl_test: classes declared in IDL, bound by somidl's C
 * emitters (ADR-0122), built by GCC or clang and run on Warpine's SOM
 * kernel. Each check is written as "som_idl_test: <name>: ok" or
 * "... FAIL", then "som_idl_test: done <passed>/<total>".
 */
#include "counter.h"
#include <stdio.h>
#include <string.h>

extern char counter_trail[32];
static int passed, total;

static void check( const char *name, int ok )
{
    total++;
    if( ok ) passed++;
    printf( "som_idl_test: %s: %s\n", name, ok ? "ok" : "FAIL" );
}

int main( void )
{
    Environment *ev = somGetGlobalEnvironment();
    Counter     *c;
    Doubler     *d;
    long         calls = -1;

    c = CounterNew();
    check( "CounterNew makes a Counter", c != 0 && strcmp( _somGetClassName( c ), "Counter" ) == 0 );
    check( "its initializer ran", strcmp( counter_trail, "C" ) == 0 );
    check( "instance data: the initial total", _peek( c, ev, &calls ) == 100 && calls == 0 );
    check( "an introduced method with an argument", _add( c, ev, 5 ) == 105 );
    check( "an out argument", _peek( c, ev, &calls ) == 105 && calls == 1 );
    _reset( c, ev );
    check( "a method without arguments", _peek( c, ev, &calls ) == 0 );
    check( "the class's version", _somCheckVersion( _Counter, 1, 2 ) && !_somCheckVersion( _Counter, 1, 3 ) );
    counter_trail[0] = '\0';
    _somFree( c );
    check( "somFree ran its destructor", strcmp( counter_trail, "c" ) == 0 );

    counter_trail[0] = '\0';
    d = DoublerNew();
    check( "a derived class: Counter's initializer first", strcmp( counter_trail, "CD" ) == 0 );
    check( "its parent is Counter", _somGetParent( _Doubler ) == _Counter );
    check( "an override calling its parent's", _add( d, ev, 5 ) == 110 );
    check( "the parent's data in a derived object", _peek( d, ev, &calls ) == 110 && calls == 1 );
    check( "the derived class's own data", _twice( d, ev ) == 1 );
    check( "Counter's method through Doubler's object", Counter_add( d, ev, 1 ) == 112 );
    counter_trail[0] = '\0';
    _somFree( d );
    check( "destructors: Doubler's, then Counter's", strcmp( counter_trail, "dc" ) == 0 );

    printf( "som_idl_test: done %d/%d\n", passed, total );
    return( passed == total ? 0 : 1 );
}
