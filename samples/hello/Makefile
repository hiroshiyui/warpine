# samples/hello/Makefile
# Relative path to the vendored Open Watcom
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom

# WCL386 (Open Watcom C/C++ 386) path
WCL386 = $(WATCOM_ROOT)/binl64/wcl386
INCLUDE = $(WATCOM_ROOT)/h/os2

# Target: hello.exe
# -l=os2v2 produces a 32-bit OS/2 Linear Executable (LX format)
# -q quiet mode
# -bt=os2 build target
all: hello.exe

hello.exe: hello.c
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Please run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE=$(INCLUDE) WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2 hello.c

clean:
	rm -f *.exe *.obj *.err
