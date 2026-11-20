/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * bird.c - class Bird in BIRD.DLL, built by somBuildClass from static
 * class information (ADR-0073). somFindClass loads the DLL and calls its
 * SOMInitModule, as for an IBM-built class library.
 */
#include "sci.h"

SOMEXTERN struct BirdClassDataStructure {
    SOMClass *classObject;
    somMToken sing;
} SOMDLINK BirdClassData;
struct BirdClassDataStructure SOMDLINK BirdClassData;
sciCClassData SOMDLINK BirdCClassData;

static long SOMLINK sing( SOMObject *somSelf ) { return 42; }

static char *bird_name = "Bird", *sing_name = "sing", *object_name = "SOMObject";
static somId bird_id = &bird_name, sing_id = &sing_name, object_id = &object_name;
static somId parents[] = { &object_name };
static sciStaticMethod methods[] = {
    { &BirdClassData.sing, &sing_id, 0, (somMethodProc *)sing, 0, 0 },
};
static sciStaticClassInfo sci = {
    3, 1, 0, 0, 0, 0, 1, 0, 0, 1, 1,
    0, 0, 0, parents, &BirdClassData, &BirdCClassData,
    methods, 0, 0, 0, 0, 0, 0,
};

SOMClass * SOMLINK BirdNewClass( integer4 major, integer4 minor )
{
    sci.classId = bird_id;
    return somBuildClass( -1, &sci, major, minor );
}

void SOMLINK SOMInitModule( integer4 major, integer4 minor, zString className )
{
    BirdNewClass( major, minor );
}
