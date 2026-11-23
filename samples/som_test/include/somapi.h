/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * somapi.h - what somidl's generated C bindings need beyond som.h, for the
 * Open Watcom samples (ADR-0122). The SDK's own somapi.h has the same, for
 * GCC and clang; the generated headers include it after som.h.
 *
 * Kept out of som.h because several samples declare some of these names
 * themselves (som_layout_test, som_build_test, som_ordinal_test).
 */
#ifndef WARPINE_SOMAPI_H
#define WARPINE_SOMAPI_H

#include <som.h>

typedef struct somMethodTabList {
    somMethodTab            *mtab;
    struct somMethodTabList *next;
} somMethodTabList, *somMethodTabs;

/* What somBuildClass takes (layout version 1, as somidl's bindings fill
 * it): a class's static methods and overrides, its parents and metaclass
 * by name (ADR-0073). */
typedef struct somStaticMethod_t {
    somMToken     *classData;       /* where its token goes */
    somId         *methodId;
    somId         *methodDescriptor;
    somMethodProc *method;
    somMethodProc *redispatchStub;
    somMethodProc *applyStub;
} somStaticMethod_t;
typedef struct somOverrideMethod_t {
    somId         *methodId;
    somMethodProc *method;
} somOverrideMethod_t;
typedef struct somStaticClassInfo {
    unsigned long        layoutVersion;
    unsigned long        numStaticMethods;
    unsigned long        numStaticOverrides;
    unsigned long        numNonInternalData;
    unsigned long        numProcMethods;
    unsigned long        numVarargsFuncs;
    unsigned long        majorVersion;
    unsigned long        minorVersion;
    unsigned long        instanceDataSize;
    unsigned long        maxMethods;
    unsigned long        numParents;
    somId                classId;
    somId                explicitMetaId;
    long                 implicitParentMeta;
    somId               *parents;
    void                *cds;            /* the class's ClassData */
    void                *ccds;           /* its CClassData */
    somStaticMethod_t   *smt;
    somOverrideMethod_t *omt;
    char                *nitReferenceBase;
    void                *nit;
    void                *pmt;
    void                *vft;
    void                *cif;
} somStaticClassInfo;

SOMEXTERN somMethodProc * SOMLINK somParentNumResolve( somMethodTabs parentMtabs, int parentNum, somMToken method );
SOMEXTERN SOMClass      * SOMLINK somBuildClass( long inheritVars, somStaticClassInfo *sci, long majorVersion, long minorVersion );
SOMEXTERN SOMClass      * SOMLINK SOMObjectNewClass( integer4 majorVersion, integer4 minorVersion );
SOMEXTERN SOMClass      * SOMLINK SOMClassNewClass( integer4 majorVersion, integer4 minorVersion );
SOMEXTERN SOMClass      * SOMLINK SOMClassMgrNewClass( integer4 majorVersion, integer4 minorVersion );
/* The ancestors class `cls`'s initializers visit, in that order, instead
 * of its parents (`directinitclasses`; `n` somIds): 0 for success. */
SOMEXTERN long          SOMLINK WrpSomSetDirectInitClasses( SOMClass *cls, long n, somId *names );


/* A parent's implementation of a method: in the `n`th parent's table of a
 * class's <Class>CClassData.parentMtab. */
#define SOM_ParentNumResolve( cls, n, mtabs, m ) \
    ((somTD_##cls##_##m) somParentNumResolve( (mtabs), (n), cls##ClassData.m ))
#define SOMMethodDebug( c, m ) SOM_MethodDebug( c, m )

#endif
