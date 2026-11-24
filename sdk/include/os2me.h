/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2me.h - a subset of the multimedia interface (MDM): the media control
 * interface's command and string calls and the structures of the basic
 * commands (ADR-0101), declared from IBM's Multimedia Programming
 * Reference. Values are Warpine's (mmpm.rs). */
#ifndef WARPINE_OS2ME_H
#define WARPINE_OS2ME_H

#include <os2.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MCIERR_SUCCESS               0
#define MCIERR_BASE                  5000
#define MCIERR_INVALID_DEVICE_ID     (MCIERR_BASE + 1)
#define MCIERR_INVALID_DEVICE_NAME   (MCIERR_BASE + 7)
#define MCIERR_CANNOT_LOAD_DRIVER    (MCIERR_BASE + 10)
#define MCIERR_UNSUPPORTED_FUNCTION  (MCIERR_BASE + 18)
#define MCIERR_FILE_NOT_FOUND        (MCIERR_BASE + 19)
#define MCIERR_OUTOFRANGE            (MCIERR_BASE + 26)
#define MCIERR_INVALID_BUFFER        (MCIERR_BASE + 50)
#define MCIERR_INVALID_FLAG          (MCIERR_BASE + 54)

#define MSV_CLOSEDCAPTION    0
#define MSV_MASTERVOLUME     1
#define MSV_HEADPHONES       2
#define MSV_SPEAKERS         3
#define MSV_WORKPATH         4
#define MSV_SYSQOSVALUE      5
#define MSV_SYSQOSERRORFLAG  6

#define MCI_OPEN    1
#define MCI_CLOSE   2
#define MCI_PLAY    4
#define MCI_SEEK    5
#define MCI_STOP    6
#define MCI_STATUS  10
#define MCI_SET     12
#define MCI_RECORD  14

#define MCI_NOTIFY             0x00000001
#define MCI_WAIT               0x00000002
#define MCI_FROM               0x00000004
#define MCI_OPEN_ELEMENT       0x00000100
#define MCI_OPEN_ALIAS         0x00000200
#define MCI_OPEN_TYPE_ID       0x00001000
#define MCI_DEVTYPE_WAVEFORM_AUDIO  7

/* Posted to the callback window when a command given MCI_NOTIFY ends. */
#define MM_MCINOTIFY           0x0500

#define MCI_NOTIFY_SUCCESSFUL  0x0000
#define MCI_NOTIFY_SUPERSEDED  0x0001
#define MCI_NOTIFY_ABORTED     0x0002
#define MCI_NOTIFY_ERROR       0x0003

#define MCI_STATUS_LENGTH    2
#define MCI_STATUS_MODE      3
#define MCI_STATUS_POSITION  5
#define MCI_MODE_NOT_READY   1
#define MCI_MODE_PLAY        3
#define MCI_MODE_STOP        4

typedef struct _MCI_GENERIC_PARMS {
    HWND hwndCallback;
} MCI_GENERIC_PARMS, *PMCI_GENERIC_PARMS;

typedef struct _MCI_OPEN_PARMS {
    HWND   hwndCallback;
    USHORT usDeviceID;
    USHORT usReserved0;
    PSZ    pszDeviceType;
    PSZ    pszElementName;
    PSZ    pszAlias;
} MCI_OPEN_PARMS, *PMCI_OPEN_PARMS;

typedef struct _MCI_PLAY_PARMS {
    HWND  hwndCallback;
    ULONG ulFrom;
    ULONG ulTo;
} MCI_PLAY_PARMS, *PMCI_PLAY_PARMS;

typedef struct _MCI_STATUS_PARMS {
    HWND  hwndCallback;
    ULONG ulReturn;
    ULONG ulItem;
    ULONG ulValue;
} MCI_STATUS_PARMS, *PMCI_STATUS_PARMS;

ULONG APIENTRY mciSendCommand(USHORT usDeviceID, USHORT usMessage, ULONG ulParam1, PVOID pParam2,
                              USHORT usUserParm);
ULONG APIENTRY mciSendString(PSZ pszCommandBuf, PSZ pszReturnString, USHORT usReturnLength, HWND hwndCallBack,
                             USHORT usUserParm);
ULONG APIENTRY mciGetErrorString(ULONG ulError, PSZ pszBuffer, USHORT usLength);
ULONG APIENTRY mciGetDeviceID(PSZ pszName);
BOOL APIENTRY mciQuerySysValue(USHORT iSysValue, PVOID pValue);
BOOL APIENTRY mciSetSysValue(USHORT iSysValue, PVOID pValue);

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_OS2ME_H */
