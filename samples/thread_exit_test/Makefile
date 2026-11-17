# samples/thread_exit_test/Makefile
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386      = $(WATCOM_ROOT)/binl64/wcl386
INCLUDE     = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h
WCLENV      = INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH)

all: thread_exit_test.exe thread_last_test.exe

thread_exit_test.exe: thread_exit_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -fe=$@ thread_exit_test.c

thread_last_test.exe: thread_exit_test.c
	$(WCLENV) $(WCL386) -q -bt=os2 -l=os2v2 -dLAST_THREAD -fo=thread_last_test.obj -fe=$@ thread_exit_test.c

clean:
	rm -f *.exe *.obj *.o *.err
