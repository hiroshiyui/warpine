# samples/pm_bitmap_test/Makefile
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386      = $(WATCOM_ROOT)/binl64/wcl386
RC          = $(WATCOM_ROOT)/binl64/wrc
INCLUDE     = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h

TARGET = pm_bitmap_test.exe
DLL    = bmpres.dll

all: $(TARGET) $(DLL)

$(TARGET): pm_bitmap_test.c pm_bitmap_test.rc quadrants.bmp
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2_pm pm_bitmap_test.c
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(RC) -bt=os2 -i=$(WATCOM_ROOT)/h/os2 -i=$(WATCOM_ROOT)/h pm_bitmap_test.rc $(TARGET)

# A resource-only DLL whose bitmap (RLE8) has the same id as the exe's.
$(DLL): bmpres.c bmpres.rc dllbmp.bmp
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bd -bt=os2 -l=os2v2_dll bmpres.c -fe=$(DLL)
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(RC) -bt=os2 -i=$(WATCOM_ROOT)/h/os2 -i=$(WATCOM_ROOT)/h bmpres.rc $(DLL)

clean:
	rm -f *.exe *.dll *.obj *.o *.res *.err
