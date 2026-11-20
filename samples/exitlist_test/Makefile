# samples/exitlist_test/Makefile
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386      = $(WATCOM_ROOT)/binl64/wcl386
INCLUDE     = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h
WCLENV      = INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH)

all: exitlist_test.exe exitlist_thread.exe exitlist_mainlast.exe exitlist_nomain.exe exitlist_hlt.exe

exitlist_test.exe: exitlist_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -fe=$@ exitlist_test.c

exitlist_thread.exe: exitlist_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -dTHREAD_EXIT -fo=exitlist_thread.obj -fe=$@ exitlist_test.c

exitlist_mainlast.exe: exitlist_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -dMAIN_LAST -fo=exitlist_mainlast.obj -fe=$@ exitlist_test.c

exitlist_nomain.exe: exitlist_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -dNO_MAIN -fo=exitlist_nomain.obj -fe=$@ exitlist_test.c

exitlist_hlt.exe: exitlist_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -dHLT_EXIT -fo=exitlist_hlt.obj -fe=$@ exitlist_test.c

clean:
	rm -f *.exe *.obj *.o *.err
