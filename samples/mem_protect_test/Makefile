# samples/mem_protect_test/Makefile
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386      = $(WATCOM_ROOT)/binl64/wcl386
INCLUDE     = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h
WCLENV      = INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH)

all: mem_protect_test.exe

mem_protect_test.exe: mem_protect_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -fe=$@ mem_protect_test.c

clean:
	rm -f *.exe *.obj *.o *.err
