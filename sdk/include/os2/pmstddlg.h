/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/pmstddlg.h - the standard file and font dialogs (PMCTLS) and drag
 * and drop (PMDRAG) (ADR-0100), declared from IBM's PM Programming
 * Reference. Included by pm.h. */
#ifndef WARPINE_OS2_PMSTDDLG_H
#define WARPINE_OS2_PMSTDDLG_H

#ifdef __cplusplus
extern "C" {
#endif

#pragma pack(push, 2)

/* ── The file dialog ────────────────────────────────────────────────── */
#define FDS_CENTER          0x00000001
#define FDS_CUSTOM          0x00000002
#define FDS_HELPBUTTON      0x00000008
#define FDS_APPLYBUTTON     0x00000010
#define FDS_MODELESS        0x00000040
#define FDS_OPEN_DIALOG     0x00000100
#define FDS_SAVEAS_DIALOG   0x00000200
#define FDS_MULTIPLESEL     0x00000400
#define FDS_ENABLEFILELB    0x00000800
#define FDS_SUCCESSFUL      0
#define FDS_ERR_INVALID_DIALOG     3
#define FDS_ERR_INVALID_DRIVE      4
#define FDS_ERR_INVALID_PATHFILE   6
#define FDS_ERR_INVALID_VERSION    10
#define FDS_ERR_DIALOG_LOAD_ERROR  12

typedef PSZ APSZ[1];
typedef APSZ *PAPSZ;

typedef struct _FILEDLG {
    ULONG   cbSize;
    ULONG   fl;
    ULONG   ulUser;
    LONG    lReturn;
    LONG    lSRC;
    PSZ     pszTitle;
    PSZ     pszOKButton;
    PFNWP   pfnDlgProc;
    PSZ     pszIType;
    PAPSZ   papszITypeList;
    PSZ     pszIDrive;
    PAPSZ   papszIDriveList;
    HMODULE hMod;
    CHAR    szFullFile[CCHMAXPATH];
    PAPSZ   papszFQFilename;
    ULONG   ulFQFCount;
    USHORT  usDlgId;
    SHORT   x;
    SHORT   y;
    SHORT   sEAType;
} FILEDLG, *PFILEDLG;

HWND    APIENTRY WinFileDlg(HWND hwndParent, HWND hwndOwner, PFILEDLG pfild);
MRESULT EXPENTRY WinDefFileDlgProc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2);
BOOL    APIENTRY WinFreeFileDlgList(PAPSZ papszFQFilename);
BOOL    APIENTRY WinRegisterCircularSlider(VOID);

/* ── The font dialog ────────────────────────────────────────────────── */
#define FNTS_CENTER            0x00000001
#define FNTS_CUSTOM            0x00000002
#define FNTS_HELPBUTTON        0x00000008
#define FNTS_APPLYBUTTON       0x00000010
#define FNTS_RESETBUTTON       0x00000020
#define FNTS_MODELESS          0x00000040
#define FNTS_INITFROMFATTRS    0x00000080
#define FNTS_FIXEDWIDTHONLY    0x00000400
#define FNTS_PROPORTIONALONLY  0x00000800
#define FNTS_SUCCESSFUL        0
#define FNTS_ERR_INVALID_DIALOG     3
#define FNTS_ERR_INVALID_VERSION    10
#define FNTS_ERR_DIALOG_LOAD_ERROR  12

typedef struct _FONTDLG {
    ULONG   cbSize;
    HPS     hpsScreen;
    HPS     hpsPrinter;
    PSZ     pszTitle;
    PSZ     pszPreview;
    PSZ     pszPtSizeList;
    PFNWP   pfnDlgProc;
    PSZ     pszFamilyname;
    FIXED   fxPointSize;
    ULONG   fl;
    ULONG   flFlags;
    ULONG   flType;
    ULONG   flTypeMask;
    ULONG   flStyle;
    ULONG   flStyleMask;
    LONG    clrFore;
    LONG    clrBack;
    ULONG   ulUser;
    LONG    lReturn;
    LONG    lSRC;
    LONG    lEmHeight;
    LONG    lXHeight;
    LONG    lExternalLeading;
    HMODULE hMod;
    FATTRS  fAttrs;
    SHORT   sNominalPointSize;
    USHORT  usWeight;
    USHORT  usWidth;
    SHORT   x;
    SHORT   y;
    USHORT  usDlgId;
    USHORT  usFamilyBufLen;
    USHORT  usReserved;
} FONTDLG, *PFONTDLG;

HWND    APIENTRY WinFontDlg(HWND hwndParent, HWND hwndOwner, PFONTDLG pfntd);
MRESULT EXPENTRY WinDefFontDlgProc(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2);

/* ── Drag and drop ──────────────────────────────────────────────────── */
typedef LHANDLE HSTR, *PHSTR;

#define DM_RENDERCOMPLETE  0x0328
#define DM_RENDER          0x0329
#define DM_DRAGLEAVE       0x032D
#define DM_DRAGOVER        0x032E
#define DM_DROP            0x032F

#define DO_COPYABLE   0x0001
#define DO_MOVEABLE   0x0002
#define DO_COPY       0x0010
#define DO_LINK       0x0018
#define DO_MOVE       0x0020
#define DO_DEFAULT    0xBFFE
#define DC_CONTAINER  0x0008

#define DOR_NODROP     0
#define DOR_DROP       1
#define DOR_NODROPOP   2
#define DOR_NEVERDROP  3

typedef struct _DRAGITEM {
    HWND   hwndItem;
    ULONG  ulItemID;
    HSTR   hstrType;
    HSTR   hstrRMF;
    HSTR   hstrContainerName;
    HSTR   hstrSourceName;
    HSTR   hstrTargetName;
    SHORT  cxOffset;
    SHORT  cyOffset;
    USHORT fsControl;
    USHORT fsSupportedOps;
} DRAGITEM, *PDRAGITEM;

typedef struct _DRAGINFO {
    ULONG  cbDraginfo;
    USHORT cbDragitem;
    USHORT usOperation;
    HWND   hwndSource;
    SHORT  xDrop;
    SHORT  yDrop;
    USHORT cditem;
    USHORT usReserved;
} DRAGINFO, *PDRAGINFO;

typedef struct _DRAGIMAGE {
    USHORT  cb;
    USHORT  cptl;
    LHANDLE hImage;
    SIZEL   sizlStretch;
    ULONG   fl;
    SHORT   cxOffset;
    SHORT   cyOffset;
} DRAGIMAGE, *PDRAGIMAGE;

typedef struct _DRAGTRANSFER {
    ULONG     cb;
    HWND      hwndClient;
    PDRAGITEM pditem;
    HSTR      hstrSelectedRMF;
    HSTR      hstrRenderToName;
    ULONG     ulTargetInfo;
    USHORT    usOperation;
    USHORT    fsReply;
} DRAGTRANSFER, *PDRAGTRANSFER;

PDRAGINFO     APIENTRY DrgAllocDraginfo(ULONG cditem);
PDRAGINFO     APIENTRY DrgReallocDraginfo(PDRAGINFO pdinfoOld, ULONG cditem);
BOOL          APIENTRY DrgFreeDraginfo(PDRAGINFO pdinfo);
BOOL          APIENTRY DrgAccessDraginfo(PDRAGINFO pdinfo);
BOOL          APIENTRY DrgPushDraginfo(PDRAGINFO pdinfo, HWND hwndDest);
BOOL          APIENTRY DrgDeleteDraginfoStrHandles(PDRAGINFO pdinfo);
PDRAGINFO     APIENTRY DrgQueryDraginfoPtr(PDRAGINFO pRsvd);
PDRAGINFO     APIENTRY DrgQueryDraginfoPtrFromHwnd(HWND hwndSource);
PDRAGINFO     APIENTRY DrgQueryDraginfoPtrFromDragitem(PDRAGITEM pditem);
ULONG         APIENTRY DrgQueryDragStatus(void);
HSTR          APIENTRY DrgAddStrHandle(PCSZ psz);
BOOL          APIENTRY DrgDeleteStrHandle(HSTR hstr);
ULONG         APIENTRY DrgQueryStrName(HSTR hstr, ULONG cbBuffer, PSZ pBuffer);
ULONG         APIENTRY DrgQueryStrNameLen(HSTR hstr);
PDRAGTRANSFER APIENTRY DrgAllocDragtransfer(ULONG cdxfer);
BOOL          APIENTRY DrgFreeDragtransfer(PDRAGTRANSFER pdxfer);
HWND          APIENTRY DrgDrag(HWND hwndSource, PDRAGINFO pdinfo, PDRAGIMAGE pdimg, ULONG cdimg, LONG vkTerminate,
                               PVOID pRsvd);
BOOL          APIENTRY DrgDragFiles(HWND hwnd, PCSZ *apszFiles, PCSZ *apszTypes, PCSZ *apszTargets, ULONG cFiles,
                                    HPOINTER hptrDrag, ULONG vkTerm, BOOL fSourceRender, ULONG ulRsvd);
HPS           APIENTRY DrgGetPS(HWND hwnd);
BOOL          APIENTRY DrgReleasePS(HPS hps);
BOOL          APIENTRY DrgPostTransferMsg(HWND hwnd, ULONG msg, PDRAGTRANSFER pdxfer, ULONG fl, ULONG ulRsvd,
                                          BOOL fRetry);
MRESULT       APIENTRY DrgSendTransferMsg(HWND hwnd, ULONG msg, MPARAM mp1, MPARAM mp2);
BOOL          APIENTRY DrgQueryDragitem(PDRAGINFO pdinfo, ULONG cbBuffer, PDRAGITEM pditem, ULONG iItem);
ULONG         APIENTRY DrgQueryDragitemCount(PDRAGINFO pdinfo);
PDRAGITEM     APIENTRY DrgQueryDragitemPtr(PDRAGINFO pdinfo, ULONG ulIndex);
BOOL          APIENTRY DrgSetDragitem(PDRAGINFO pdinfo, PDRAGITEM pditem, ULONG cbBuffer, ULONG iItem);
BOOL          APIENTRY DrgSetDragPointer(PDRAGINFO pdinfo, HPOINTER hptr);
BOOL          APIENTRY DrgSetDragImage(PDRAGINFO pdinfo, PDRAGIMAGE pdimg, ULONG cdimg, PVOID pRsvd);
BOOL          APIENTRY DrgQueryNativeRMF(PDRAGITEM pditem, ULONG cbBuffer, PCHAR pBuffer);
ULONG         APIENTRY DrgQueryNativeRMFLen(PDRAGITEM pditem);
BOOL          APIENTRY DrgQueryTrueType(PDRAGITEM pditem, ULONG cbBuffer, PSZ pBuffer);
ULONG         APIENTRY DrgQueryTrueTypeLen(PDRAGITEM pditem);
BOOL          APIENTRY DrgVerifyNativeRMF(PDRAGITEM pditem, PCSZ pszRMF);
BOOL          APIENTRY DrgVerifyRMF(PDRAGITEM pditem, PCSZ pszMech, PCSZ pszFmt);
BOOL          APIENTRY DrgVerifyTrueType(PDRAGITEM pditem, PCSZ pszType);
BOOL          APIENTRY DrgVerifyType(PDRAGITEM pditem, PCSZ pszType);
BOOL          APIENTRY DrgVerifyTypeSet(PDRAGITEM pditem, PCSZ pszType, ULONG cbMatch, PSZ pszMatch);

#pragma pack(pop)

#ifdef __cplusplus
}
#endif

#endif /* WARPINE_OS2_PMSTDDLG_H */
