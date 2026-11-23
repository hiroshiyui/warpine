/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * somcdev.h - the macros C bindings of SOM classes use (ADR-0122)
 *
 * Written for Warpine from the SOM Programming Guide's description of the
 * C bindings; som.h includes it after the root classes. The initializer
 * and destructor macros implement Warpine's own protocol (somapi.h), not
 * IBM's, whose control-block layout is not published.
 */
#ifndef WARPINE_SOMCDEV_H
#define WARPINE_SOMCDEV_H

#ifndef WARPINE_SOM_H
#error "include <som.h>, which includes somcdev.h"
#endif

#define SOM_Scope static

/* A method of class `cls` for object `o`, through its token in
 * <cls>ClassData; a class's own implementation of it; a parent's (its
 * <cls>CClassData.parentMtab list, parents from 1). */
#define SOM_Resolve( o, cls, m ) \
    ((somTD_##cls##_##m) somResolve( (SOMObject *)(o), cls##ClassData.m ))
#define SOM_ClassResolve( c, cls, m ) \
    ((somTD_##cls##_##m) somClassResolve( (SOMClass *)(c), cls##ClassData.m ))
#define SOM_ParentNumResolve( cls, n, mtabs, m ) \
    ((somTD_##cls##_##m) somParentNumResolve( (mtabs), (n), cls##ClassData.m ))

/* Method tracing: a line per method entered while SOM_TraceLevel is set. */
#define SOM_NoTrace( c, m ) ((void)0)
#define SOMMethodDebug( c, m ) \
    do { if( SOM_TraceLevel ) somPrintf( "\"%s\": %s\n", (c), (m) ); } while( 0 )

/* ---- Warpine's initializer protocol ----
 * In class C's somDefaultInit( SOMObject *somSelf, somInitCtrl *ctrl ):
 *     SOM_InitBegin( ctrl, C's class object );
 *     ... initialize C's own data ...
 *     SOM_InitEnd();
 * and in its somDestruct( somSelf, octet doFree, somDestructCtrl *ctrl ):
 *     SOM_DestructBegin( ctrl, doFree, C's class object );
 *     ... release C's own data ...
 *     SOM_DestructEnd( ctrl, doFree );
 * Each ancestor's own initializer runs once, ancestors first; destructors
 * run in the reverse order, then the memory goes when doFree is set. */

/* The entry of class `cls` in `ctrl`'s table (its count if none). */
static __inline__ long WrpSomCtrlIndex( somInitCtrl *ctrl, SOMClass *cls )
{
    long i;
    for( i = 0; i < ctrl->count; i++ )
        if( ctrl->info[i].cls == cls ) return( i );
    return( ctrl->count );
}

/* Run the initializers of the entries before `cls`'s still flagged. */
static __inline__ void WrpSomInitAncestors( SOMObject *self, somInitCtrl *ctrl, SOMClass *cls )
{
    long i, n;
    if( ctrl == 0 ) return;
    n = WrpSomCtrlIndex( ctrl, cls );
    for( i = 0; i < n; i++ ) {
        if( !ctrl->mask[i] ) continue;
        ctrl->mask[i] = 0;
        if( ctrl->info[i].defaultInit )
            ((somTD_SOMObject_somDefaultInit)ctrl->info[i].defaultInit)( self, ctrl );
    }
    if( n < ctrl->count ) ctrl->mask[n] = 0;
}

/* Run the destructors of the entries before `cls`'s still flagged, last
 * first. */
static __inline__ void WrpSomDestructAncestors( SOMObject *self, somDestructCtrl *ctrl, SOMClass *cls )
{
    long i;
    if( ctrl == 0 ) return;
    i = WrpSomCtrlIndex( ctrl, cls );
    if( i < ctrl->count ) ctrl->mask[i] = 0;
    while( i-- > 0 ) {
        if( !ctrl->mask[i] ) continue;
        ctrl->mask[i] = 0;
        if( ctrl->info[i].destruct )
            ((somTD_SOMObject_somDestruct)ctrl->info[i].destruct)( self, 0, ctrl );
    }
}

/* Without a control block, a class's initializer runs the protocol for
 * the object; an inherited one (the object's class is a descendant that
 * has none of its own) hands the whole walk to the kernel, so every class
 * of a multiple-inheritance hierarchy still gets its turn. */
#define SOM_InitBegin( ctrl, cls ) \
    somInitCtrl *somOwnCtrl_ = 0; \
    if( (ctrl) == 0 && _somGetClass( somSelf ) != (SOMClass *)(cls) ) { \
        WrpSomInitWalk( (SOMObject *)somSelf ); \
        return; \
    } \
    if( (ctrl) == 0 ) (ctrl) = somOwnCtrl_ = WrpSomInitCtrlNew( (SOMClass *)(cls) ); \
    WrpSomInitAncestors( (SOMObject *)somSelf, (ctrl), (SOMClass *)(cls) )
#define SOM_InitEnd() \
    do { if( somOwnCtrl_ ) WrpSomInitCtrlFree( somOwnCtrl_ ); } while( 0 )

#define SOM_DestructBegin( ctrl, doFree, cls ) \
    somDestructCtrl *somOwnCtrl_ = 0; \
    SOMClass *somCtrlClass_ = (SOMClass *)(cls); \
    if( (ctrl) == 0 && _somGetClass( somSelf ) != somCtrlClass_ ) { \
        WrpSomDestructWalk( (SOMObject *)somSelf, (doFree) ); \
        return; \
    } \
    if( (ctrl) == 0 ) (ctrl) = somOwnCtrl_ = WrpSomInitCtrlNew( somCtrlClass_ )
#define SOM_DestructEnd( ctrl, doFree ) \
    do { \
        WrpSomDestructAncestors( (SOMObject *)somSelf, (ctrl), somCtrlClass_ ); \
        if( somOwnCtrl_ ) WrpSomInitCtrlFree( somOwnCtrl_ ); \
        if( doFree ) _somDeallocate( _somGetClass( somSelf ), somSelf ); \
    } while( 0 )

#endif
