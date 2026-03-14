# samples/4os2/Makefile
# Cross-compile 4OS2 from Linux using Open Watcom v2
#
# Builds only the main 4os2.exe (no DLLs, resources, help, or 16-bit stubs).

WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCC386 = $(WATCOM_ROOT)/binl64/wcc386
WASM = $(WATCOM_ROOT)/binl64/wasm
WLINK = $(WATCOM_ROOT)/binl64/wlink

# Environment for Watcom tools
export WATCOM = $(WATCOM_ROOT)
export INCLUDE = $(WATCOM_ROOT)/h:$(WATCOM_ROOT)/h/os2:$(CURDIR)/h
export PATH := $(WATCOM_ROOT)/binl64:$(PATH)

# Compiler flags (adapted from master.mif)
# -zq quiet, -wx max warnings, -bc console app, -s no stack checks
# -ef for source file in errors, -bt=os2 target OS/2
# Removed -we (warnings as errors) to allow building despite minor warnings
CFLAGS = -zq -wx -s -bt=os2 -bc -I$(CURDIR)/h -I$(WATCOM_ROOT)/h/os2
AFLAGS = -zq -bt=os2

# Release optimization flags
CFLAGS += -oaxt

# Object directory
OBJDIR = build

# Source files for 4os2.exe (from master.mif)
4OS2_CSRC = main.c parser.c os2init.c cmds.c expand.c eval.c \
            ea.c dircmds.c filecmds.c syscmds.c os2cmds.c \
            os2calls.c misc.c env.c batch.c screenio.c msg.c \
            error.c window.c listall.c lines.c kparse.c \
            iofmt32.c select.c os2tty.c istring.c iparse.c \
            dbcs.c wrappers.c

4OS2_ASMSRC = bcd32.asm os32util.asm

4OS2_COBJ = $(patsubst %.c,$(OBJDIR)/%.obj,$(4OS2_CSRC))
4OS2_ASMOBJ = $(patsubst %.asm,$(OBJDIR)/%.obj,$(4OS2_ASMSRC))
4OS2_OBJ = $(OBJDIR)/crt0.obj $(OBJDIR)/viowrap.obj $(4OS2_COBJ) $(4OS2_ASMOBJ)

all: $(OBJDIR) 4os2.exe

$(OBJDIR):
	mkdir -p $(OBJDIR)

# Compile viowrap.c and crt0.c WITHOUT -fi (they provide imports/startup, not app code)
$(OBJDIR)/viowrap.obj: c/viowrap.c
	$(WCC386) -zq -s -bt=os2 -fo=$@ $<

$(OBJDIR)/crt0.obj: c/crt0.c
	$(WCC386) -zq -s -bt=os2 -I$(WATCOM_ROOT)/h/os2 -fo=$@ $<

# Compile C sources
$(OBJDIR)/%.obj: c/%.c
	$(WCC386) $(CFLAGS) -fo=$@ $<

# Assemble ASM sources
$(OBJDIR)/%.obj: a/%.asm
	$(WASM) $(AFLAGS) -fo=$@ $<

# Link 4os2.exe
# We create a linker script inline since the original .wlk references
# vdm2os2.exe (16-bit DOS stub) and buildlevel.txt via REXX paths.
# Link 4os2.exe
# Link os2386.lib FIRST for 32-bit VIO/KBD imports (bypasses __vfthunk),
# then os2.lib for remaining 16-bit APIs (DosGetInfoSeg, DosQFileMode, etc.).
# The linker resolves symbols from the first library that provides them,
# so os2386.lib's 32-bit VIO/KBD symbols override clib3r.lib's __vfthunk versions.
4os2.exe: $(4OS2_OBJ) buildlevel.txt
	$(WLINK) \
		name $@ \
		system os2v2 \
		option map \
		option quiet \
		option description @buildlevel.txt \
		library $(WATCOM_ROOT)/lib386/os2/os2386 \
		import VIO16GETMODE_ VIOCALLS.3 \
		import VIO16GETCURPOS_ VIOCALLS.4 \
		import VIO16SETCURPOS_ VIOCALLS.15 \
		import VIO16WRTTTY_ VIOCALLS.30 \
		import VIO16SCROLLUP_ VIOCALLS.7 \
		import VIO16SCROLLDN_ VIOCALLS.8 \
		import VIO16WRTCHARSTRATT_ VIOCALLS.26 \
		import VIO16WRTNCELL_ VIOCALLS.28 \
		import VIO16WRTNATTR_ VIOCALLS.27 \
		import VIO16READCELLSTR_ VIOCALLS.24 \
		import VIO16SETCURTYPE_ VIOCALLS.16 \
		import VIO16SETSTATE_ VIOCALLS.51 \
		import VIO16GETCONFIG_ VIOCALLS.46 \
		import VIO16SETANSI_ VIOCALLS.38 \
		import VIO16GETANSI_ VIOCALLS.39 \
		import VIO16SETCP_ VIOCALLS.42 \
		import KBD16CHARIN_ KBDCALLS.4 \
		import KBD16GETSTATUS_ KBDCALLS.10 \
		import KBD16STRINGIN_ KBDCALLS.9 \
		import DOSGETINFOSEG DOSCALLS.8 \
		import DOSQFILEMODE DOSCALLS.75 \
		import DOSSETFILEMODE DOSCALLS.76 \
		file { $(4OS2_OBJ) }

clean:
	rm -rf $(OBJDIR) *.exe *.map *.err

.PHONY: all clean
