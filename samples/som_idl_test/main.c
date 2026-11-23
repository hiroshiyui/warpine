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

    /* Stage S3: two parents, directinitclasses, an init method. */
    {
        Both *b, *copy;
        counter_trail[0] = '\0';
        b = BothNew();
        check( "two parents: directinitclasses order (Tagged, then Counter)", strcmp( counter_trail, "TCB" ) == 0 );
        check( "the first parent's method and data", _add( b, ev, 1 ) == 101 );
        check( "the second parent's method and data", _tag( b, ev ) == 42 );
        check( "its own data", _start( b, ev ) == -1 );
        check( "Both descends from both", _somIsA( b, _Counter ) && _somIsA( b, _Tagged ) );
        counter_trail[0] = '\0';
        copy = (Both *)_somNewNoInit( _Both );
        Both_withStart( copy, ev, 0, 7 );
        check( "an init method: the ancestors' initializers, then its own", strcmp( counter_trail, "TCW" ) == 0 );
        check( "the init method's argument", _start( copy, ev ) == 7 );
        _somDefaultCopyInit( copy, 0, b );
        check( "somDefaultCopyInit copies the instance", _start( copy, ev ) == -1 && _peek( copy, ev, &calls ) == 101 );
        _add( b, ev, 9 );
        check( "somDefaultAssign returns the object", _somDefaultAssign( copy, 0, b ) == (SOMObject *)copy );
        check( "... with the other's data", _peek( copy, ev, &calls ) == 110 );
        _somFree( copy );
        _somFree( b );
    }

    /* Stage S4: exceptions, a union, an any. */
    {
        Gauge        *g = GaugeNew();
        Gauge_Reading r;
        any           a;
        check( "no exception: the value", _set( g, ev, 5 ) == 5 && ev->_major == NO_EXCEPTION );
        _set( g, ev, 500 );
        check( "a raised exception: USER_EXCEPTION", ev->_major == USER_EXCEPTION );
        check( "... its id", somExceptionId( ev ) && strcmp( somExceptionId( ev ), ex_TooBig ) == 0 );
        check( "... its value", ((TooBig *)somExceptionValue( ev ))->limit == 100 );
        somExceptionFree( ev );
        check( "somExceptionFree clears it", ev->_major == NO_EXCEPTION && somExceptionId( ev ) == 0 );
        _set( g, ev, -3 );
        check( "an interface's exception: its scoped id", strcmp( somExceptionId( ev ), "Gauge::Negative" ) == 0
               && ((Gauge_Negative *)somExceptionValue( ev ))->value == -3 );
        SOM_UninitEnvironment( ev );
        r = _read( g, ev, 2 );
        check( "a union: the discriminator and its arm", r._d == 2 && r._u.scaled == 2.5 );
        a = _boxed( g, ev );
        check( "an any: its value", *(long *)a._value == 5 );
        _somFree( g );
    }

    printf( "som_idl_test: done %d/%d\n", passed, total );
    return( passed == total ? 0 : 1 );
}
