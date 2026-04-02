# Makefile for pm_menu_dlg_test
# Requires Open Watcom (setup via ../../vendor/setup_watcom.sh)

CC      = wcc386
RC      = rc
LINK    = wlink
CFLAGS  = -bt=os2 -I$(WATCOM)/h -I$(WATCOM)/h/os2 -mf -3s -zq

TARGET  = pm_menu_dlg_test.exe
OBJ     = pm_menu_dlg_test.obj
RES     = pm_menu_dlg_test.res

all: $(TARGET)

$(OBJ): pm_menu_dlg_test.c
	$(CC) $(CFLAGS) $<

$(RES): pm_menu_dlg_test.rc
	$(RC) -r $<

$(TARGET): $(OBJ) $(RES)
	$(LINK) system os2v2 pmcompatible \
	    name $(TARGET) \
	    file $(OBJ) \
	    library $(WATCOM)/lib386/os2/os2386.lib
	$(RC) $(RES) $(TARGET)

clean:
	rm -f $(OBJ) $(RES) $(TARGET)
