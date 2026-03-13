# samples/ipc_test/Makefile
# Relative path to the vendored Open Watcom
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom

# WCL386 (Open Watcom C/C++ 386) path
WCL386 = $(WATCOM_ROOT)/binl64/wcl386
# Multiple include paths for OS/2 and standard headers
INCLUDE = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h

all: ipc_test.exe

ipc_test.exe: ipc_test.c
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Please run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2 ipc_test.c

clean:
	rm -f *.exe *.obj *.err
