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

/* Tagged: a second base class with data and an initializer. */
SOM_Scope long SOMLINK t_tag( Tagged *somSelf, Environment *ev )
{
    TaggedData *somThis = TaggedGetData( somSelf );
    (void)ev;
    return( _t );
}

SOM_Scope void SOMLINK t_somDefaultInit( Tagged *somSelf, somInitCtrl *ctrl )
{
    TaggedData *somThis = TaggedGetData( somSelf );
    SOM_InitBegin( ctrl, _Tagged );
    _t = 42;
    note( 'T' );
    SOM_InitEnd();
}

/* Both: two parents; its initializers visit Tagged before Counter. */
SOM_Scope void SOMLINK b_withStart( Both *somSelf, Environment *ev, somInitCtrl *ctrl, long start )
{
    BothData *somThis = BothGetData( somSelf );
    (void)ev;
    SOM_InitBegin( ctrl, _Both );
    _s = start;
    note( 'W' );
    SOM_InitEnd();
}

SOM_Scope long SOMLINK b_start( Both *somSelf, Environment *ev )
{
    BothData *somThis = BothGetData( somSelf );
    (void)ev;
    return( _s );
}

SOM_Scope void SOMLINK b_somDefaultInit( Both *somSelf, somInitCtrl *ctrl )
{
    BothData *somThis = BothGetData( somSelf );
    SOM_InitBegin( ctrl, _Both );
    _s = -1;
    note( 'B' );
    SOM_InitEnd();
}

/* Gauge: exceptions through the Environment, a union, an any. */
SOM_Scope long SOMLINK g_set( Gauge *somSelf, Environment *ev, long v )
{
    GaugeData *somThis = GaugeGetData( somSelf );
    if( v < 0 ) {
        Gauge_Negative *e = (Gauge_Negative *)SOMMalloc( sizeof( *e ) );
        e->value = v;
        somSetException( ev, USER_EXCEPTION, ex_Gauge_Negative, e );
        return( 0 );
    }
    if( v > 100 ) {
        TooBig *e = (TooBig *)SOMMalloc( sizeof( *e ) );
        e->limit = 100;
        somSetException( ev, USER_EXCEPTION, ex_TooBig, e );
        return( 0 );
    }
    return( _level = v );
}

SOM_Scope Gauge_Reading SOMLINK g_read( Gauge *somSelf, Environment *ev, long kind )
{
    GaugeData *somThis = GaugeGetData( somSelf );
    Gauge_Reading r;
    (void)ev;
    r._d = kind;
    if( kind == 1 ) r._u.whole = _level;
    else if( kind == 2 || kind == 3 ) r._u.scaled = _level / 2.0;
    else r._u.text = "unknown";
    return( r );
}

SOM_Scope any SOMLINK g_boxed( Gauge *somSelf, Environment *ev )
{
    GaugeData *somThis = GaugeGetData( somSelf );
    any a;
    (void)ev;
    a._type = 0;
    a._value = &_level;
    return( a );
}
