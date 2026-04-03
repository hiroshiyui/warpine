# samples/socket_test/Makefile
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386      = $(WATCOM_ROOT)/binl64/wcl386
INCLUDE     = $(WATCOM_ROOT)/h/os2:$(WATCOM_ROOT)/h

all: socket_test.exe

socket_test.exe: socket_test.c
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2 socket_test.c

clean:
	rm -f *.exe *.obj *.err
