# samples/som_test/Makefile
#
# The Open Watcom SOM samples (vendor/watcom/samples/os2/som), built
# unchanged against Warpine's SOM runtime (ADR-0059). Their C sources come
# straight from the Open Watcom tree; the bindings the SOM compiler would
# generate from their .idl / .csc files (hello.h, hello.ih, student.h …)
# are written by hand here, over include/som.h.
#
#   helloc/  -> hello.exe    a class with one method
#   classes/ -> classes.exe  four classes in four DLLs: inheritance,
#                            overrides, parent calls, instance data
#   animals/ -> animals.exe  metaclasses with data and constructors, a
#                            derived metaclass, attributes, sequences,
#                            somPrintf and SOMMalloc

WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
SOMSAMPLES  = $(WATCOM_ROOT)/samples/os2/som
WCC386      = $(WATCOM_ROOT)/binl64/wcc386
WLINK       = $(WATCOM_ROOT)/binl64/wlink

export WATCOM  = $(WATCOM_ROOT)
export INCLUDE = $(WATCOM_ROOT)/h/os2:$(WATCOM_ROOT)/h
export PATH   := $(WATCOM_ROOT)/binl64:$(PATH)

B = build
CFLAGS = -zq -bt=os2 -I$(CURDIR)/include

all: hello.exe classes.exe animals.exe

$(B):
	mkdir -p $(B)

# ---- helloc ----
$(B)/hello.obj: $(SOMSAMPLES)/helloc/hello.c hello/hello.h hello/hello.ih | $(B)
	$(WCC386) $(CFLAGS) -I$(CURDIR)/hello -fo=$@ $<
$(B)/hmain.obj: $(SOMSAMPLES)/helloc/main.c hello/hello.h | $(B)
	$(WCC386) $(CFLAGS) -I$(CURDIR)/hello -fo=$@ $<
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

# ---- animals ----
ANI = -I$(CURDIR)/animals
$(B)/animals.obj: $(SOMSAMPLES)/animals/animals.c animals/animals.h animals/animals.ih | $(B)
	$(WCC386) $(CFLAGS) $(ANI) -fo=$@ $<
$(B)/amain.obj: $(SOMSAMPLES)/animals/main.c animals/animals.h | $(B)
	$(WCC386) $(CFLAGS) $(ANI) -fo=$@ $<
animals.exe: $(B)/animals.obj $(B)/amain.obj som.lnk
	$(WLINK) name $@ system os2v2 option quiet @som.lnk file { $(B)/animals.obj $(B)/amain.obj }

clean:
	rm -rf $(B) *.exe *.dll *.err

.PHONY: all clean
