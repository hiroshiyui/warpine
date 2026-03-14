# samples/ne_hello/Makefile — 16-bit OS/2 NE format hello world
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL = $(WATCOM_ROOT)/binl64/wcl
INCLUDE = $(WATCOM_ROOT)/h/os21x

all: ne_hello.exe

ne_hello.exe: ne_hello.c
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Please run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL) -q -bt=os2 -l=os2 ne_hello.c

clean:
	rm -f *.exe *.obj *.err *.map
