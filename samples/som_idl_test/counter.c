/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * counter.c - the implementation of counter.idl's classes, over the
 * bindings somidl generates (counter.ih). The initializers and destructors
 * note their class in a trail, so the test sees the protocol's order.
 */
#include "counter.ih"
#include <string.h>

char counter_trail[32];

static void note( char c )
{
    size_t n = strlen( counter_trail );
    if( n + 1 < sizeof( counter_trail ) ) { counter_trail[n] = c; counter_trail[n + 1] = '\0'; }
}

SOM_Scope long SOMLINK c_add( Counter *somSelf, Environment *ev, long n )
{
    CounterData *somThis = CounterGetData( somSelf );
    (void)ev;
    CounterMethodDebug( "Counter", "add" );
    _calls++;
    return( _total += n );
}

SOM_Scope void SOMLINK c_reset( Counter *somSelf, Environment *ev )
{
    CounterData *somThis = CounterGetData( somSelf );
    (void)ev;
    _total = 0;
}

SOM_Scope long SOMLINK c_peek( Counter *somSelf, Environment *ev, long *calls )
{
    CounterData *somThis = CounterGetData( somSelf );
    (void)ev;
    *calls = _calls;
    return( _total );
}

SOM_Scope void SOMLINK c_somDefaultInit( Counter *somSelf, somInitCtrl *ctrl )
{
    CounterData *somThis = CounterGetData( somSelf );
    SOM_InitBegin( ctrl, _Counter );
    _total = 100;
    _calls = 0;
    note( 'C' );
    SOM_InitEnd();
}

SOM_Scope void SOMLINK c_somDestruct( Counter *somSelf, octet doFree, somDestructCtrl *ctrl )
{
    SOM_DestructBegin( ctrl, doFree, _Counter );
    note( 'c' );
    SOM_DestructEnd( ctrl, doFree );
}

/* Doubler: add counts twice, through Counter's add. */
SOM_Scope long SOMLINK d_add( Doubler *somSelf, Environment *ev, long n )
{
    DoublerData *somThis = DoublerGetData( somSelf );
    _doubled++;
    return( Doubler_parent_Counter_add( somSelf, ev, 2 * n ) );
}

SOM_Scope long SOMLINK d_twice( Doubler *somSelf, Environment *ev )
{
    DoublerData *somThis = DoublerGetData( somSelf );
    (void)ev;
    return( _doubled );
}

SOM_Scope void SOMLINK d_somDefaultInit( Doubler *somSelf, somInitCtrl *ctrl )
{
    DoublerData *somThis = DoublerGetData( somSelf );
    SOM_InitBegin( ctrl, _Doubler );
    _doubled = 0;
    note( 'D' );
    SOM_InitEnd();
}

SOM_Scope void SOMLINK d_somDestruct( Doubler *somSelf, octet doFree, somDestructCtrl *ctrl )
{
    SOM_DestructBegin( ctrl, doFree, _Doubler );
    note( 'd' );
    SOM_DestructEnd( ctrl, doFree );
}
