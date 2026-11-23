# samples/som_test/Makefile
#
# The Open Watcom SOM samples (vendor/watcom/samples/os2/som), built
# unchanged against Warpine's SOM runtime (ADR-0059). Their C sources come
# straight from the Open Watcom tree. hello's and animals' bindings are
# generated from their .idl by somidl (ADR-0122; cargo build makes it);
# the classes' (from OIDL .csc files, which somidl does not read) are
# written by hand here, over include/som.h.
#
#   helloc/  -> hello.exe    a class with one method (somidl's bindings)
#   classes/ -> classes.exe  four classes in four DLLs: inheritance,
#                            overrides, parent calls, instance data
#   animals/ -> animals.exe  (somidl's bindings) metaclasses with data and constructors, a
#                            derived metaclass, attributes, sequences,
#                            somPrintf and SOMMalloc
#   hellocpp/ -> hellocpp.exe the C++ bindings: a SOM class as a C++
#                            class, new through somNewNoInit and
#                            somDefaultInit (ADR-0061)
#   findcls/ -> findcls.exe  Warpine's own check of class loading:
#                            somFindClass loads greeter.dll / echoes.dll
#                            by class name; the replaceable SOMLoadModule,
#                            SOMOutCharRoutine and SOMError (ADR-0062)
#   cpp/     -> sompp.exe    Warpine's own C++ check: a class derived
#                            from hellocpp's Hello (its object reused),
#                            overrides, parent calls, delete

WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
SOMSAMPLES  = $(WATCOM_ROOT)/samples/os2/som
WCC386      = $(WATCOM_ROOT)/binl64/wcc386
WPP386      = $(WATCOM_ROOT)/binl64/wpp386
WLINK       = $(WATCOM_ROOT)/binl64/wlink

export WATCOM  = $(WATCOM_ROOT)
export INCLUDE = $(WATCOM_ROOT)/h/os2:$(WATCOM_ROOT)/h
export PATH   := $(WATCOM_ROOT)/binl64:$(PATH)

B = build
CFLAGS = -zq -bt=os2 -I$(CURDIR)/include

all: hello.exe classes.exe animals.exe hellocpp.exe sompp.exe findcls.exe greeter.dll echoes.dll

$(B):
	mkdir -p $(B)

# Every object follows the shared bindings: a change to them rebuilds it.
$(wildcard $(B)/*.obj): include/som.h include/som.xh

# ---- helloc: its bindings generated from hello.idl by somidl (ADR-0122) ----
# Where cargo put somidl (CARGO_TARGET_DIR, as cargo reads it; CI's is
# target/ci).
CARGO_TARGET_DIR ?= $(CURDIR)/../../target
SOMIDL ?= $(CARGO_TARGET_DIR)/debug/somidl
$(B)/hello/hello.h $(B)/hello/hello.ih &: $(SOMSAMPLES)/helloc/hello.idl $(SOMIDL) $(wildcard ../../sdk/include/*.idl) | $(B)
	mkdir -p $(B)/hello
	$(SOMIDL) -I $(CURDIR)/../../sdk/include -s h,ih -o $(B)/hello $<
$(B)/hello.obj: $(SOMSAMPLES)/helloc/hello.c $(B)/hello/hello.h $(B)/hello/hello.ih | $(B)
	$(WCC386) $(CFLAGS) -I$(CURDIR)/$(B)/hello -fo=$@ $<
$(B)/hmain.obj: $(SOMSAMPLES)/helloc/main.c $(B)/hello/hello.h | $(B)
	$(WCC386) $(CFLAGS) -I$(CURDIR)/$(B)/hello -fo=$@ $<
hello.exe: $(B)/hello.obj $(B)/hmain.obj som.lnk
	$(WLINK) name $@ system os2v2 option quiet @som.lnk file { $(B)/hello.obj $(B)/hmain.obj }

# ---- classes: one DLL per class, as the Open Watcom makefile builds them ----
CLS = -I$(CURDIR)/classes
$(B)/%.obj: $(SOMSAMPLES)/classes/%.c $(wildcard classes/*.h classes/*.ih) | $(B)
	$(WCC386) $(CFLAGS) $(CLS) -bd -fo=$@ $<

student.dll: $(B)/student.obj som.lnk
	$(WLINK) name $@ system os2v2_dll option quiet option implib=$(B)/student.lib @som.lnk \
		file $< export StudentCClassData,StudentClassData,StudentNewClass
undgrad.dll: $(B)/undgrad.obj student.dll som.lnk
	$(WLINK) name $@ system os2v2_dll option quiet option implib=$(B)/undgrad.lib @som.lnk \
		file $< library $(B)/student.lib \
		export UnderGraduateStudentCClassData,UnderGraduateStudentClassData,UnderGraduateStudentNewClass
graduate.dll: $(B)/graduate.obj student.dll som.lnk
	$(WLINK) name $@ system os2v2_dll option quiet option implib=$(B)/graduate.lib @som.lnk \
		file $< library $(B)/student.lib \
		export GraduateStudentCClassData,GraduateStudentClassData,GraduateStudentNewClass
course.dll: $(B)/course.obj student.dll som.lnk
	$(WLINK) name $@ system os2v2_dll option quiet option implib=$(B)/course.lib @som.lnk \
		file $< library $(B)/student.lib export CourseCClassData,CourseClassData,CourseNewClass

$(B)/test.obj: $(SOMSAMPLES)/classes/test.c $(wildcard classes/*.h) | $(B)
	$(WCC386) $(CFLAGS) $(CLS) -fo=$@ $<
classes.exe: $(B)/test.obj student.dll undgrad.dll graduate.dll course.dll som.lnk
	$(WLINK) name $@ system os2v2 option quiet @som.lnk file $< \
		library { $(B)/student.lib $(B)/course.lib $(B)/graduate.lib $(B)/undgrad.lib }

# ---- animals: its bindings generated from animals.idl by somidl ----
$(B)/animals/animals.h $(B)/animals/animals.ih &: $(SOMSAMPLES)/animals/animals.idl $(SOMIDL) $(wildcard ../../sdk/include/*.idl) | $(B)
	mkdir -p $(B)/animals
	$(SOMIDL) -I $(CURDIR)/../../sdk/include -s h,ih -o $(B)/animals $<
ANI = -I$(CURDIR)/$(B)/animals
$(B)/animals.obj: $(SOMSAMPLES)/animals/animals.c $(B)/animals/animals.h $(B)/animals/animals.ih | $(B)
	$(WCC386) $(CFLAGS) $(ANI) -fo=$@ $<
$(B)/amain.obj: $(SOMSAMPLES)/animals/main.c $(B)/animals/animals.h | $(B)
	$(WCC386) $(CFLAGS) $(ANI) -fo=$@ $<
animals.exe: $(B)/animals.obj $(B)/amain.obj som.lnk
	$(WLINK) name $@ system os2v2 option quiet @som.lnk file { $(B)/animals.obj $(B)/amain.obj }

# ---- hellocpp: the C++ bindings ----
HPP = -I$(CURDIR)/hellocpp
$(B)/hellocpp.obj: $(SOMSAMPLES)/hellocpp/hello.cpp hellocpp/hello.xh hellocpp/hello.xih include/som.xh | $(B)
	$(WPP386) $(CFLAGS) $(HPP) -fo=$@ $<
$(B)/hcmain.obj: $(SOMSAMPLES)/hellocpp/main.cpp hellocpp/hello.xh include/som.xh | $(B)
	$(WPP386) $(CFLAGS) $(HPP) -fo=$@ $<
hellocpp.exe: $(B)/hellocpp.obj $(B)/hcmain.obj som.lnk
	$(WLINK) name $@ system os2v2 option quiet @som.lnk file { $(B)/hellocpp.obj $(B)/hcmain.obj }

# ---- cpp: Warpine's own C++ check ----
CPP = $(HPP) -I$(CURDIR)/cpp
$(B)/loud.obj: cpp/loud.cpp cpp/loud.xh cpp/loud.xih hellocpp/hello.xh include/som.xh | $(B)
	$(WPP386) $(CFLAGS) $(CPP) -fo=$@ $<
$(B)/sompp.obj: cpp/sompp.cpp cpp/loud.xh hellocpp/hello.xh include/som.xh | $(B)
	$(WPP386) $(CFLAGS) $(CPP) -fo=$@ $<
sompp.exe: $(B)/hellocpp.obj $(B)/loud.obj $(B)/sompp.obj som.lnk
	$(WLINK) name $@ system os2v2 option quiet @som.lnk file { $(B)/hellocpp.obj $(B)/loud.obj $(B)/sompp.obj }

# ---- findcls: class loading; the DLLs are not imported by the program ----
FND = -I$(CURDIR)/findcls
$(B)/greeter.obj: findcls/greeter.c include/som.h | $(B)
	$(WCC386) $(CFLAGS) $(FND) -bd -fo=$@ $<
$(B)/echo.obj: findcls/echo.c include/som.h | $(B)
	$(WCC386) $(CFLAGS) $(FND) -bd -fo=$@ $<
greeter.dll: $(B)/greeter.obj som.lnk
	$(WLINK) name $@ system os2v2_dll option quiet @som.lnk file $< \
		export SOMInitModule,GreeterNewClass,GreeterClassData
echoes.dll: $(B)/echo.obj som.lnk
	$(WLINK) name $@ system os2v2_dll option quiet @som.lnk file $< export EchoNewClass,EchoClassData
$(B)/findcls.obj: findcls/findcls.c include/som.h | $(B)
	$(WCC386) $(CFLAGS) $(FND) -fo=$@ $<
findcls.exe: $(B)/findcls.obj greeter.dll echoes.dll som.lnk
	$(WLINK) name $@ system os2v2 option quiet @som.lnk file $(B)/findcls.obj

clean:
	rm -rf $(B) *.exe *.dll *.err

.PHONY: all clean
