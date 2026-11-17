# samples/pm_paint_test/Makefile
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386 = $(WATCOM_ROOT)/binl64/wcl386
INCLUDE = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h

all: pm_paint_test.exe

pm_paint_test.exe: pm_paint_test.c
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2_pm pm_paint_test.c

clean:
	rm -f *.exe *.obj *.o *.err
