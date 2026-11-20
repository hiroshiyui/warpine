/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * undgrad.h - C usage bindings of class UnderGraduateStudent (undgrad.csc of the Open Watcom SOM
 * samples), written by hand for Warpine as the SOM compiler would emit
 * them (ADR-0059).
 */
#ifndef SOM_UnderGraduateStudent_h
#define SOM_UnderGraduateStudent_h

#include "student.h"

typedef Student UnderGraduateStudent;

typedef void SOMLINK somTP_UnderGraduateStudent_setUpUnderGraduateStudent( UnderGraduateStudent *somSelf, char *id, char *name, char *date );
typedef somTP_UnderGraduateStudent_setUpUnderGraduateStudent *somTD_UnderGraduateStudent_setUpUnderGraduateStudent;

SOMEXTERN SOMClass * SOMLINK UnderGraduateStudentNewClass( integer4 major, integer4 minor );

SOMEXTERN struct UnderGraduateStudentClassDataStructure {
    SOMClass *classObject;
    somMToken setUpUnderGraduateStudent;
} SOMDLINK UnderGraduateStudentClassData;

#define _UnderGraduateStudent  (UnderGraduateStudentClassData.classObject ? UnderGraduateStudentClassData.classObject : UnderGraduateStudentNewClass( 0, 0 ))
#define UnderGraduateStudentNew()  ((UnderGraduateStudent *)_somNew( _UnderGraduateStudent ))

#define _setUpUnderGraduateStudent( somSelf, id, name, date ) \
    SOM_Resolve( somSelf, UnderGraduateStudent, setUpUnderGraduateStudent )( (UnderGraduateStudent *)(somSelf), id, name, date )

#endif
