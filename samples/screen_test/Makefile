# samples/screen_test/Makefile — VIO screen mode verification test
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386 = $(WATCOM_ROOT)/binl64/wcl386
# Use 4OS2's patched bsesub.h (APIENTRY16 → _System) to avoid 16-bit thunks
INCLUDE = $(CURDIR)/../4os2/h:$(WATCOM_ROOT)/h/os2

all: screen_test.exe

screen_test.exe: screen_test.c
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Please run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2 -fm=screen_test.map screen_test.c

clean:
	rm -f *.exe *.obj *.err
