# samples/pm_child_dialog_test/Makefile
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386      = $(WATCOM_ROOT)/binl64/wcl386
RC          = $(WATCOM_ROOT)/binl64/wrc
INCLUDE     = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h

TARGET = pm_child_dialog_test.exe

all: $(TARGET)

$(TARGET): pm_child_dialog_test.c pm_child_dialog_test.rc pm_child_dialog_test.h
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2_pm pm_child_dialog_test.c
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(RC) -q -bt=os2 -i=$(WATCOM_ROOT)/h/os2 -i=$(WATCOM_ROOT)/h pm_child_dialog_test.rc $(TARGET)

clean:
	rm -f *.exe *.obj *.o *.res *.err
