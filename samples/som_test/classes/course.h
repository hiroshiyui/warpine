/*
 * course.h - C usage bindings of class Course (course.csc of the Open
 * Watcom SOM samples), written by hand for Warpine as the SOM compiler
 * would emit them (ADR-0059).
 */
#ifndef SOM_Course_h
#define SOM_Course_h

#include "student.h"

typedef SOMObject Course;

typedef void SOMLINK somTP_Course_setUpCourse( Course *somSelf, char *code, char *title,
                                               char *instructor, int credit, int capacity );
typedef int  SOMLINK somTP_Course_addStudent( Course *somSelf, Student *student );
typedef void SOMLINK somTP_Course_dropStudent( Course *somSelf, char *studentId );
typedef void SOMLINK somTP_Course_printCourseInfo( Course *somSelf );
typedef somTP_Course_setUpCourse     *somTD_Course_setUpCourse;
typedef somTP_Course_addStudent      *somTD_Course_addStudent;
typedef somTP_Course_dropStudent     *somTD_Course_dropStudent;
typedef somTP_Course_printCourseInfo *somTD_Course_printCourseInfo;

SOMEXTERN SOMClass * SOMLINK CourseNewClass( integer4 major, integer4 minor );

SOMEXTERN struct CourseClassDataStructure {
    SOMClass *classObject;
    somMToken setUpCourse, addStudent, dropStudent, printCourseInfo;
} SOMDLINK CourseClassData;

#define _Course  (CourseClassData.classObject ? CourseClassData.classObject : CourseNewClass( 0, 0 ))
#define CourseNew()  ((Course *)_somNew( _Course ))

#define _setUpCourse( somSelf, code, title, instructor, credit, capacity ) \
    SOM_Resolve( somSelf, Course, setUpCourse )( (Course *)(somSelf), (code), (title), \
                                                 (instructor), (credit), (capacity) )
#define _addStudent( somSelf, student ) \
    SOM_Resolve( somSelf, Course, addStudent )( (Course *)(somSelf), (student) )
#define _dropStudent( somSelf, studentId ) \
    SOM_Resolve( somSelf, Course, dropStudent )( (Course *)(somSelf), (studentId) )
#define _printCourseInfo( somSelf ) \
    SOM_Resolve( somSelf, Course, printCourseInfo )( (Course *)(somSelf) )

#endif
