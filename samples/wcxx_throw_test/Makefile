# samples/wcxx_throw_test/Makefile - Open Watcom C++ with exceptions (-xs)
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386      = $(WATCOM_ROOT)/binl64/wcl386
INCLUDE     = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h
WCLENV      = INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH)

all: wcxx_throw_test.exe

wcxx_throw_test.exe: wcxx_throw_test.cpp
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -xs -fe=$@ wcxx_throw_test.cpp

clean:
	rm -f *.exe *.obj *.o *.err
