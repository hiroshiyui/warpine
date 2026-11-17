# samples/pm_pointer_test/Makefile
WATCOM_ROOT = $(CURDIR)/../../vendor/watcom
WCL386      = $(WATCOM_ROOT)/binl64/wcl386
RC          = $(WATCOM_ROOT)/binl64/wrc
INCLUDE     = $(WATCOM_ROOT)/h/os2;$(WATCOM_ROOT)/h

TARGET = pm_pointer_test.exe

all: $(TARGET)

# The pointer files are written by make_ptr.py from the documented format.
cross.ptr array.ptr: make_ptr.py
	python3 -I make_ptr.py .

$(TARGET): pm_pointer_test.c pm_pointer_test.rc pm_pointer_test.h cross.ptr array.ptr
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(WCL386) -q -bt=os2 -l=os2v2_pm pm_pointer_test.c
	INCLUDE="$(INCLUDE)" WATCOM=$(WATCOM_ROOT) PATH=$(WATCOM_ROOT)/binl64:$(PATH) \
	$(RC) -q -bt=os2 -i=$(WATCOM_ROOT)/h/os2 -i=$(WATCOM_ROOT)/h pm_pointer_test.rc $(TARGET)

clean:
	rm -f *.exe *.obj *.o *.res *.err *.ptr
