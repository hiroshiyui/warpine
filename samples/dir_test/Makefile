# samples/dir_test/Makefile — 32-bit OS/2 directory listing test
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386 = $(WATCOM_ROOT)/binl64/wcl386
INCLUDE = $(WATCOM_ROOT)/h/os2

all: dir_test.exe

dir_test.exe: dir_test.c
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Please run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2 dir_test.c

clean:
	rm -f *.exe *.obj *.err
