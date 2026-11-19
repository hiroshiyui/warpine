# samples/dllinit_test/Makefile
#
# DLL initialization and module names (ADR-0063): goodinit.dll / badinit.dll
# with Open Watcom's __dll_initialize, dllinit.exe and badstart.exe.

WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCC386      = $(WATCOM_ROOT)/binl64/wcc386
WLINK       = $(WATCOM_ROOT)/binl64/wlink

export WATCOM  = $(WATCOM_ROOT)
export INCLUDE = $(WATCOM_ROOT)/h/os2:$(WATCOM_ROOT)/h
export PATH   := $(WATCOM_ROOT)/binl64:$(PATH)

CFLAGS = -zq -bt=os2

all: dllinit.exe badstart.exe

goodinit.obj: goodinit.c
	$(WCC386) $(CFLAGS) -bd -fo=$@ $<
badinit.obj: badinit.c
	$(WCC386) $(CFLAGS) -bd -fo=$@ $<
goodinit.dll: goodinit.obj
	$(WLINK) name $@ system os2v2_dll initinstance terminstance option quiet option implib=goodinit.lib \
		file $< export GoodValue
badinit.dll: badinit.obj
	$(WLINK) name $@ system os2v2_dll initinstance terminstance option quiet option implib=badinit.lib \
		file $< export BadValue
dllinit.obj: dllinit.c
	$(WCC386) $(CFLAGS) -fo=$@ $<
badstart.obj: badstart.c
	$(WCC386) $(CFLAGS) -fo=$@ $<
dllinit.exe: dllinit.obj goodinit.dll badinit.dll
	$(WLINK) name $@ system os2v2 option quiet file $< library goodinit.lib
badstart.exe: badstart.obj badinit.dll
	$(WLINK) name $@ system os2v2 option quiet file $< library badinit.lib

clean:
	rm -f *.obj *.exe *.dll *.lib *.err

.PHONY: all clean
