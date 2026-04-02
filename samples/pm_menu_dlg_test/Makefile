# samples/pm_menu_dlg_test/Makefile
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386      = $(WATCOM_ROOT)/binl64/wcl386
RC          = $(WATCOM_ROOT)/binl64/wrc
INCLUDE     = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h

TARGET = pm_menu_dlg_test.exe

all: $(TARGET)

$(TARGET): pm_menu_dlg_test.c pm_menu_dlg_test.rc
	@if [ ! -d "$(WATCOM_ROOT)" ]; then \
		echo "Error: Open Watcom not found. Run ../../vendor/setup_watcom.sh first."; \
		exit 1; \
	fi
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2_pm pm_menu_dlg_test.c
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(RC) -bt=os2 -i=$(WATCOM_ROOT)/h/os2 -i=$(WATCOM_ROOT)/h pm_menu_dlg_test.rc $(TARGET)

clean:
	rm -f *.exe *.obj *.res *.err
