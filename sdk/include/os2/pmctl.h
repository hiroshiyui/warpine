/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/pmctl.h - the standard controls' messages, flags and structures
 * (containers, notebooks, sliders, spin buttons, value sets, scroll bars,
 * combination boxes, the MLE, owner drawing), DDE and error values for
 * GCC and clang (ADR-0101). Included by pm.h. Values are Warpine's own
 * constants where it has them (`gen_api check-headers` keeps them equal)
 * and the PM Programming Reference's otherwise. */
#ifndef WARPINE_OS2_PMCTL_H
#define WARPINE_OS2_PMCTL_H

#pragma pack(push, 2)

/* Containers: messages, notifications, attributes and views */
#define CA_CONTAINERTITLE             0x0200
#define CA_TITLESEPARATOR             0x0400
#define CA_TITLELEFT                  0x0800
#define CA_TITLERIGHT                 0x1000
#define CA_OWNERDRAW                  0x4000
#define CA_DETAILSVIEWTITLES          0x8000
#define CA_DRAWBITMAP                 0x00020000
#define CA_DRAWICON                   0x00040000
#define CA_TITLEREADONLY              0x00080000
#define CA_ORDEREDTARGETEMPH          0x00010000
#define CA_MIXEDTARGETEMPH            0x00200000
#define CA_TREELINE                   0x00400000
#define CFA_RIGHT                     0x0002
#define CFA_CENTER                    0x0004
#define CFA_INVISIBLE                 0x0040
#define CFA_BITMAPORICON              0x0100
#define CFA_SEPARATOR                 0x0200
#define CFA_HORZSEPARATOR             0x0400
#define CFA_STRING                    0x0800
#define CFA_OWNER                     0x1000
#define CFA_DATE                      0x2000
#define CFA_TIME                      0x4000
#define CFA_FIREADONLY                0x8000
#define CFA_FITITLEREADONLY           0x00010000
#define CFA_ULONG                     0x00020000
#define CID_LEFTCOLTITLEWND           0x7FF0
#define CID_RIGHTCOLTITLEWND          0x7FF1
#define CID_HSCROLL                   0x7FF3
#define CID_RIGHTHSCROLL              0x7FF4
#define CID_CNRTITLEWND               0x7FF5
#define CID_LEFTDVWND                 0x7FF7
#define CID_RIGHTDVWND                0x7FF8
#define CID_VSCROLL                   0x7FF9
#define CID_MLE                       0x7FFA
#define CM_MODE1                      0x0001
#define CM_MODE3                      0x0003
#define CM_ALLOCDETAILFIELDINFO       0x0330
#define CM_ALLOCRECORD                0x0331
#define CM_ARRANGE                    0x0332
#define CM_ERASERECORD                0x0333
#define CM_FILTER                     0x0334
#define CM_FREEDETAILFIELDINFO        0x0335
#define CM_FREERECORD                 0x0336
#define CM_INSERTDETAILFIELDINFO      0x0338
#define CM_INSERTRECORD               0x0339
#define CM_INVALIDATEDETAILFIELDINFO  0x033A
#define CM_INVALIDATERECORD           0x033B
#define CM_QUERYCNRINFO               0x033D
#define CM_QUERYDETAILFIELDINFO       0x033E
#define CM_QUERYRECORD                0x0340
#define CM_QUERYRECORDEMPHASIS        0x0341
#define CM_QUERYRECORDFROMRECT        0x0342
#define CM_QUERYRECORDRECT            0x0343
#define CM_QUERYVIEWPORTRECT          0x0344
#define CM_REMOVEDETAILFIELDINFO      0x0345
#define CM_REMOVERECORD               0x0346
#define CM_SCROLLWINDOW               0x0347
#define CM_SEARCHSTRING               0x0348
#define CM_SETCNRINFO                 0x0349
#define CM_SETRECORDEMPHASIS          0x034A
#define CM_SORTRECORD                 0x034B
#define CM_OPENEDIT                   0x034C
#define CM_CLOSEEDIT                  0x034D
#define CM_COLLAPSETREE               0x034E
#define CM_EXPANDTREE                 0x034F
#define CM_QUERYRECORDINFO            0x0350
#define CM_INSERTRECORDARRAY          0x0351
#define CM_MOVETREE                   0x0352
#define CM_SETGRIDINFO                0x0354
#define CM_QUERYGRIDINFO              0x0355
#define CMA_ARRANGESTANDARD           0x0000
#define CMA_ARRANGEGRID               0x0001
#define CMA_CNRTITLE                  0x0001
#define CMA_FREE                      0x0001
#define CMA_ITEMORDER                 0x0001
#define CMA_PARENT                    0x0001
#define CMA_TOP                       0x0001
#define CMA_ARRANGESELECTED           0x0002
#define CMA_BOTTOM                    0x0002
#define CMA_DELTA                     0x0002
#define CMA_FIRSTCHILD                0x0002
#define CMA_WINDOW                    0x0002
#define CMA_FLWINDOWATTR              0x0004
#define CMA_LASTCHILD                 0x0004
#define CMA_WORKSPACE                 0x0004
#define CMA_LINESPACING               0x0008
#define CMA_ZORDER                    0x0008
#define CMA_FIRST                     0x0010
#define CMA_PFIELDINFOLAST            0x0010
#define CMA_LAST                      0x0020
#define CMA_PSORTRECORD               0x0020
#define CMA_END                       0x0040
#define CMA_PTLORIGIN                 0x0040
#define CMA_PREV                      0x0080
#define CMA_SLBITMAPORICON            0x0080
#define CMA_NEXT                      0x0100
#define CMA_XVERTSPLITBAR             0x0100
#define CMA_HORIZONTAL                0x0200
#define CMA_PFIELDINFOOBJECT          0x0200
#define CMA_TREEICON                  0x0400
#define CMA_VERTICAL                  0x0400
#define CMA_ICON                      0x0800
#define CMA_TREEBITMAP                0x0800
#define CMA_CXTREEINDENT              0x1000
#define CMA_FILTER                    0x1000
#define CMA_TEXT                      0x1000
#define CMA_CXTREELINE                0x2000
#define CMA_PARTIAL                   0x2000
#define CMA_COMPLETE                  0x4000
#define CMA_SLTREEBITMAPORICON        0x4000
#define CN_DRAGAFTER                  0x0065
#define CN_DRAGLEAVE                  0x0066
#define CN_DRAGOVER                   0x0067
#define CN_DROP                       0x0068
#define CN_ENTER                      0x006A
#define CN_INITDRAG                   0x006B
#define CN_EMPHASIS                   0x006C
#define CN_KILLFOCUS                  0x006D
#define CN_SCROLL                     0x006E
#define CN_SETFOCUS                   0x0070
#define CN_REALLOCPSZ                 0x0071
#define CN_BEGINEDIT                  0x0072
#define CN_ENDEDIT                    0x0073
#define CN_COLLAPSETREE               0x0074
#define CN_EXPANDTREE                 0x0075
#define CN_HELP                       0x0076
#define CN_CONTEXTMENU                0x0077
#define CRA_SELECTED                  0x0001
#define CRA_TARGET                    0x0002
#define CRA_CURSORED                  0x0004
#define CRA_FILTERED                  0x0010
#define CRA_RECORDREADONLY            0x0040
#define CRA_EXPANDED                  0x0080
#define CRA_COLLAPSED                 0x0100
#define CRA_DISABLED                  0x1000
#define CV_TEXT                       0x0001
#define CV_NAME                       0x0002
#define CV_ICON                       0x0004
#define CV_DETAIL                     0x0008
#define CV_FLOW                       0x0010
#define CV_MINI                       0x0020
#define CV_TREE                       0x0040

/* Notebooks */
#define BDS_HILITED                   0x0100
#define BDS_DISABLED                  0x0200
#define BDS_DEFAULT                   0x0400
#define BFA_PAGEDATA                  0x0001
#define BFA_PAGEFROMHWND              0x0002
#define BFA_PAGEFROMDLGTEMPLATE       0x0004
#define BFA_PAGEFROMDLGRES            0x0008
#define BFA_STATUSLINE                0x0010
#define BFA_MAJORTABBITMAP            0x0020
#define BFA_MINORTABBITMAP            0x0040
#define BFA_MAJORTABTEXT              0x0080
#define BFA_MINORTABTEXT              0x0100
#define BKA_ALL                       0x0001
#define BKA_BACKGROUNDPAGECOLORINDEX  0x0001
#define BKA_MAJORTAB                  0x0001
#define BKA_STATUSTEXTON              0x0001
#define BKA_LAST                      0x0002
#define BKA_MINORTAB                  0x0002
#define BKA_SINGLE                    0x0002
#define BKA_FIRST                     0x0004
#define BKA_TAB                       0x0004
#define BKA_NEXT                      0x0008
#define BKA_PREV                      0x0010
#define BKA_TOP                       0x0020
#define BKA_MAJOR                     0x0040
#define BKA_MINOR                     0x0080
#define BKA_AUTOPAGESIZE              0x0100
#define BKA_PAGEBUTTON                0x0100
#define BKA_MAXBUTTONID               0x1F3F
#define BKM_CALCPAGERECT              0x0353
#define BKM_DELETEPAGE                0x0354
#define BKM_INSERTPAGE                0x0355
#define BKM_INVALIDATETABS            0x0356
#define BKM_TURNTOPAGE                0x0357
#define BKM_QUERYPAGECOUNT            0x0358
#define BKM_QUERYPAGEID               0x0359
#define BKM_QUERYPAGEDATA             0x035A
#define BKM_QUERYPAGEWINDOWHWND       0x035B
#define BKM_QUERYTABBITMAP            0x035C
#define BKM_QUERYTABTEXT              0x035D
#define BKM_SETDIMENSIONS             0x035E
#define BKM_SETPAGEDATA               0x035F
#define BKM_SETPAGEWINDOWHWND         0x0360
#define BKM_SETSTATUSLINETEXT         0x0361
#define BKM_SETTABBITMAP              0x0362
#define BKM_SETTABTEXT                0x0363
#define BKM_SETNOTEBOOKCOLORS         0x0364
#define BKM_QUERYPAGESTYLE            0x0365
#define BKM_QUERYSTATUSLINETEXT       0x0366
#define BKM_SETPAGEINFO               0x0367
#define BKM_QUERYPAGEINFO             0x0368
#define BKM_SETTABCOLOR               0x0374
#define BKM_SETNOTEBOOKBUTTONS        0x0375
#define BKN_PAGESELECTED              0x0082
#define BKN_NEWPAGESIZE               0x0083
#define BKN_PAGEDELETED               0x0085
#define BKN_PAGESELECTEDPENDING       0x0086

/* Sliders */
#define SDA_RIBBONSTRIP          0x0001
#define SDA_SLIDERSHAFT          0x0002
#define SDA_BACKGROUND           0x0003
#define SDA_SLIDERARM            0x0004
#define SLM_ADDDETENT            0x0369
#define SLM_QUERYDETENTPOS       0x036A
#define SLM_QUERYSCALETEXT       0x036B
#define SLM_QUERYSLIDERINFO      0x036C
#define SLM_QUERYTICKPOS         0x036D
#define SLM_QUERYTICKSIZE        0x036E
#define SLM_REMOVEDETENT         0x036F
#define SLM_SETSCALETEXT         0x0370
#define SLM_SETSLIDERINFO        0x0371
#define SLM_SETTICKSIZE          0x0372
/* Circular sliders (WC_CIRCULARSLIDER) */
#define CSM_QUERYRANGE           0x053D
#define CSM_SETRANGE             0x053E
#define CSM_QUERYVALUE           0x053F
#define CSM_SETVALUE             0x0540
#define CSM_QUERYRADIUS          0x0541
#define CSM_SETINCREMENT         0x0542
#define CSM_QUERYINCREMENT       0x0543
#define CSM_SETBITMAPDATA        0x0544
#define CSN_SETFOCUS             0x0548
#define CSN_CHANGED              0x0549
#define CSN_TRACKING             0x054A
#define CSN_QUERYBACKGROUNDCOLOR 0x054B
#define CSS_NOBUTTON             0x0001
#define CSS_NOTEXT               0x0002
#define CSS_NONUMBER             0x0004
#define CSS_POINTSELECT          0x0008
#define CSS_360                  0x0010
#define CSS_MIDPOINT             0x0020
#define CSS_PROPORTIONALTICKS    0x0040
#define CSS_NOTICKS              0x0080
#define CSS_CIRCULARVALUE        0x0100
#define SLN_CHANGE               0x0001
#define SLN_SLIDERTRACK          0x0002
#define SLN_SETFOCUS             0x0003
#define SLN_KILLFOCUS            0x0004
#define SMA_RANGEVALUE           0x0000
#define SMA_SHAFTDIMENSIONS      0x0000
#define SMA_INCREMENTVALUE       0x0001
#define SMA_SCALE1               0x0001
#define SMA_SHAFTPOSITION        0x0001
#define SMA_SCALE2               0x0002
#define SMA_SLIDERARMDIMENSIONS  0x0002
#define SMA_SLIDERARMPOSITION    0x0003
#define SMA_SETALLTICKS          0xFFFF

/* Spin buttons */
#define SPBM_OVERRIDESETLIMITS  0x0200
#define SPBM_QUERYLIMITS        0x0201
#define SPBM_SETTEXTLIMIT       0x0202
#define SPBM_SPINUP             0x0203
#define SPBM_SPINDOWN           0x0204
#define SPBM_QUERYVALUE         0x0205
#define SPBM_SETARRAY           0x0206
#define SPBM_SETLIMITS          0x0207
#define SPBM_SETCURRENTVALUE    0x0208
#define SPBM_SETMASTER          0x0209
#define SPBN_UPARROW            0x020A
#define SPBN_DOWNARROW          0x020B
#define SPBN_ENDSPIN            0x020C
#define SPBN_CHANGE             0x020D
#define SPBN_SETFOCUS           0x020E
#define SPBN_KILLFOCUS          0x020F
#define SPBQ_UPDATEIFVALID      0x0000
#define SPBQ_ALWAYSUPDATE       0x0001
#define SPBQ_DONOTUPDATE        0x0003

/* Value sets */
#define VDA_ITEM              0x0001
#define VDA_ITEMBACKGROUND    0x0002
#define VDA_SURROUNDING       0x0003
#define VDA_BACKGROUND        0x0004
#define VIA_BITMAP            0x0001
#define VIA_ICON              0x0002
#define VIA_TEXT              0x0004
#define VIA_RGB               0x0008
#define VIA_COLORINDEX        0x0010
#define VIA_OWNERDRAW         0x0020
#define VIA_DISABLED          0x0040
#define VIA_DRAGGABLE         0x0080
#define VIA_DROPONABLE        0x0100
#define VM_QUERYITEM          0x0375
#define VM_QUERYITEMATTR      0x0376
#define VM_QUERYMETRICS       0x0377
#define VM_QUERYSELECTEDITEM  0x0378
#define VM_SELECTITEM         0x0379
#define VM_SETITEM            0x037A
#define VM_SETITEMATTR        0x037B
#define VM_SETMETRICS         0x037C
#define VMA_ITEMSIZE          0x0001
#define VMA_ITEMSPACING       0x0002
#define VN_SELECT             0x0078
#define VN_ENTER              0x0079
#define VN_DRAGLEAVE          0x007A
#define VN_DRAGOVER           0x007B
#define VN_DROP               0x007C
#define VN_INITDRAG           0x007E
#define VN_SETFOCUS           0x007F
#define VN_KILLFOCUS          0x0080
#define VN_HELP               0x0081

/* Scroll bars, combination boxes, the MLE, list search */
#define CBID_LIST          0x029A
#define CBID_EDIT          0x029B
#define CBN_EFCHANGE       0x0001
#define CBN_EFSCROLL       0x0002
#define CBN_MEMERROR       0x0003
#define CBN_LBSELECT       0x0004
#define CBN_LBSCROLL       0x0005
#define CBN_SHOWLIST       0x0006
#define CBN_ENTER          0x0007
#define LSS_SUBSTRING      0x0001
#define LSS_PREFIX         0x0002
#define LSS_CASESENSITIVE  0x0004
#define MLM_SETWRAP        0x01B4
#define MLM_QUERYWRAP      0x01B5
#define MLM_SETREADONLY    0x01B8
#define MLM_QUERYREADONLY  0x01B9
#define MLM_SETSEL         0x01CA
#define MLM_QUERYSEL       0x01CB
#define MLM_CUT            0x01D8
#define MLM_COPY           0x01D9
#define MLM_PASTE          0x01DA
#define MLM_CLEAR          0x01DB
#define MLN_CHANGE         0x0007
#define MLN_SETFOCUS       0x0008
#define MLN_KILLFOCUS      0x0009
#define SBM_SETSCROLLBAR   0x01A0
#define SBM_SETPOS         0x01A1
#define SBM_QUERYPOS       0x01A2
#define SBM_QUERYRANGE     0x01A3
#define SBM_SETTHUMBSIZE   0x01A6

/* DDE */
#define DDEPM_RETRY         0x0001
#define DDEPM_NOFREE        0x0002
#define WM_DDE_FIRST        0x00A0
#define WM_DDE_INITIATE     0x00A0
#define WM_DDE_REQUEST      0x00A1
#define WM_DDE_ACK          0x00A2
#define WM_DDE_DATA         0x00A3
#define WM_DDE_ADVISE       0x00A4
#define WM_DDE_UNADVISE     0x00A5
#define WM_DDE_POKE         0x00A6
#define WM_DDE_EXECUTE      0x00A7
#define WM_DDE_TERMINATE    0x00A8
#define WM_DDE_INITIATEACK  0x00A9
#define WM_DDE_LAST         0x00AF

/* Messages, cursors, drawing, dialogs */
#define BMSG_POST             0x0000
#define BMSG_SEND             0x0001
#define BMSG_POSTQUEUE        0x0002
#define BMSG_DESCENDANTS      0x0004
#define BMSG_FRAMEONLY        0x0008
#define CURSOR_SOLID          0x0000
#define CURSOR_HALFTONE       0x0001
#define CURSOR_FRAME          0x0002
#define CURSOR_FLASH          0x0004
#define CURSOR_BIDI_FIRST     0x0100
#define CURSOR_BIDI_LAST      0x0200
#define CURSOR_SETPOS         0x8000
#define DB_PATCOPY            0x0000
#define DB_PATINVERT          0x0001
#define DB_DESTINVERT         0x0002
#define DB_AREAMIXMODE        0x0003
#define DB_ROP                0x0007
#define DB_INTERIOR           0x0008
#define DB_AREAATTRS          0x0010
#define DB_STANDARD           0x0100
#define DB_DLGBORDER          0x0200
#define DBM_INVERT            0x0001
#define DBM_STRETCH           0x0004
#define DBM_XOR               0x0010
#define DCE_POSTONE           0x0800
#define DCE_AUTORESET         0x1000
#define DLGC_ENTRYFIELD       0x0001
#define DLGC_BUTTON           0x0002
#define DLGC_RADIOBUTTON      0x0004
#define DLGC_STATIC           0x0008
#define DLGC_DEFAULT          0x0010
#define DLGC_PUSHBUTTON       0x0020
#define DLGC_CHECKBOX         0x0040
#define DLGC_SCROLLBAR        0x0080
#define DLGC_MENU             0x0100
#define DLGC_MLE              0x0400
#define DP_HALFTONED          0x0001
#define DP_INVERTED           0x0002
#define EDI_FIRSTTABITEM      0x0000
#define EDI_LASTTABITEM       0x0001
#define EDI_NEXTTABITEM       0x0002
#define EDI_PREVTABITEM       0x0003
#define EDI_FIRSTGROUPITEM    0x0004
#define EDI_LASTGROUPITEM     0x0005
#define EDI_NEXTGROUPITEM     0x0006
#define EDI_PREVGROUPITEM     0x0007
#define HMQ_CURRENT           0x0001
#define HT_ERROR              (-3)
#define HT_DISCARD            (-2)
#define HT_TRANSPARENT        (-1)
#define HT_NORMAL             0x0000
#define PSF_LOCKWINDOWUPDATE  0x0001
#define PSF_CLIPUPWARDS       0x0002
#define PSF_CLIPDOWNWARDS     0x0004
#define PSF_CLIPSIBLINGS      0x0008
#define PSF_CLIPCHILDREN      0x0010
#define PSF_PARENTCLIP        0x0020
#define QPF_NOINHERIT         0x0001
#define QPF_ID1COLORINDEX     0x0002
#define QPF_ID2COLORINDEX     0x0004
#define QPF_PURERGBCOLOR      0x0008
#define QS_KEY                0x0001
#define QS_MOUSEBUTTON        0x0002
#define QS_MOUSEMOVE          0x0004
#define QS_TIMER              0x0008
#define QS_PAINT              0x0010
#define QS_POSTMSG            0x0020
#define QS_SEM1               0x0040
#define QS_SEM2               0x0080
#define QS_SEM3               0x0100
#define QS_SEM4               0x0200
#define QS_SENDMSG            0x0400
#define QWP_PFNWP             (-3)
#define RECTDIR_LFRT_TOPBOT   0x0001
#define RECTDIR_RTLF_TOPBOT   0x0002
#define RECTDIR_LFRT_BOTTOP   0x0003
#define RECTDIR_RTLF_BOTTOP   0x0004
#define SW_SCROLLCHILDREN     0x0001
#define SW_INVALIDATERGN      0x0002
#define TF_LEFT               0x0001
#define TF_TOP                0x0002
#define TF_RIGHT              0x0004
#define TF_BOTTOM             0x0008
#define TF_MOVE               0x000F
#define TF_SETPOINTERPOS      0x0010
#define TF_GRID               0x0020
#define TF_STANDARD           0x0040
#define TF_ALLINBOUNDARY      0x0080
#define TF_VALIDATETRACKRECT  0x0100
#define TF_PARTINBOUNDARY     0x0200

/* Errors */
#define ERRACT_RETRY                     0x0001
#define ERRACT_DLYRET                    0x0002
#define ERRACT_USER                      0x0003
#define ERRACT_ABORT                     0x0004
#define ERRCLASS_OUTRES                  0x0001
#define ERRCLASS_TEMPSIT                 0x0002
#define ERRCLASS_AUTH                    0x0003
#define ERRCLASS_INTRN                   0x0004
#define ERRCLASS_APPERR                  0x0007
#define ERRCLASS_NOTFND                  0x0008
#define ERRCLASS_BADFMT                  0x0009
#define ERRCLASS_LOCKED                  0x000A
#define ERRCLASS_ALREADY                 0x000C
#define ERRCLASS_UNK                     0x000D
#define ERRLOC_UNK                       0x0001
#define ERRLOC_DISK                      0x0002
#define ERRLOC_MEM                       0x0005
#define PMERR_INVALID_HWND               0x1001
#define PMERR_INVALID_HMQ                0x1002
#define PMERR_PARAMETER_OUT_OF_RANGE     0x1003
#define PMERR_RESOURCE_NOT_FOUND         0x100A
#define PMERR_INVALID_STRING_PARM        0x100B
#define PMERR_HEAP_OUT_OF_MEMORY         0x1011
#define PMERR_INVALID_HATOMTBL           0x1013
#define PMERR_INVALID_ATOM               0x1014
#define PMERR_INVALID_ATOM_NAME          0x1015
#define PMERR_INVALID_INTEGER_ATOM       0x1016
#define PMERR_ATOM_NAME_NOT_FOUND        0x1017
#define PMERR_INVALID_FLAG               0x1019
#define PMERR_INVALID_HACCEL             0x101A
#define PMERR_INVALID_HPTR               0x101B
#define PMERR_INVALID_HENUM              0x101C
#define PMERR_NO_MSG_QUEUE               0x1036
#define PMERR_MSG_QUEUE_ALREADY_EXISTS   0x1052
#define PMERR_BUFFER_TOO_SMALL           0x110B
#define PMERR_INVALID_INI_FILE_HANDLE    0x1115
#define PMERR_INI_FILE_IS_SYS_OR_USER    0x1124
#define PMERR_NO_SPACE                   0x1201
#define PMERR_INVALID_SWITCH_HANDLE      0x1202
#define PMERR_NO_HANDLE                  0x1203
#define PMERR_INVALID_WINDOW             0x1206
#define PMERR_INVALID_PARAMETERS         0x1208
#define PMERR_OPENING_INI_FILE           0x1301
#define PMERR_INI_FILE_CORRUPT           0x1302
#define PMERR_INVALID_PARM               0x1303
#define PMERR_NOT_IN_IDX                 0x1304
#define PMERR_INI_WRITE_FAIL             0x1306
#define PMERR_SOURCE_SAME_AS_TARGET      0x1502
#define PMERR_INVALID_PROGRAM_CATEGORY   0x152F
#define PMERR_INVALID_APPL               0x1530
#define PMERR_CANNOT_START               0x1531
#define PMERR_INVALID_HAPP               0x1533
#define PMERR_NOT_DRAGGING               0x1F00
#define PMERR_ALREADY_DRAGGING           0x1F01
#define PMERR_RECORD_CURRENTLY_INSERTED  0x1F04
#define PMERR_FI_CURRENTLY_INSERTED      0x1F05
#define PMERR_ALREADY_IN_AREA            0x2001
#define PMERR_ALREADY_IN_PATH            0x2003
#define PMERR_COORDINATE_OVERFLOW        0x2014
#define PMERR_DRIVER_NOT_FOUND           0x2026
#define PMERR_HRGN_BUSY                  0x2034
#define PMERR_INSUFFICIENT_MEMORY        0x203E
#define PMERR_INV_AREA_CONTROL           0x2041
#define PMERR_INV_BACKGROUND_MIX_ATTR    0x2045
#define PMERR_INV_CHAR_ANGLE_ATTR        0x204B
#define PMERR_INV_CHAR_DIRECTION_ATTR    0x204C
#define PMERR_INV_CHAR_MODE_ATTR         0x204D
#define PMERR_INV_CHAR_POS_OPTIONS       0x204E
#define PMERR_INV_CHAR_SHEAR_ATTR        0x2050
#define PMERR_INV_CLIP_PATH_OPTIONS      0x2051
#define PMERR_INV_COORD_SPACE            0x205A
#define PMERR_INV_COORDINATE             0x205B
#define PMERR_INV_DC_TYPE                0x2060
#define PMERR_INV_DRAW_BORDER_OPTION     0x2068
#define PMERR_INV_FILL_PATH_OPTIONS      0x2070
#define PMERR_INV_FIRST_CHAR             0x2071
#define PMERR_INV_GRAPHICS_FIELD         0x207A
#define PMERR_INV_HBITMAP                0x207B
#define PMERR_INV_HDC                    0x207C
#define PMERR_INV_HPS                    0x207F
#define PMERR_INV_HRGN                   0x2080
#define PMERR_INV_ID                     0x2081
#define PMERR_INV_IN_AREA                0x2085
#define PMERR_INV_IN_PATH                0x208B
#define PMERR_INV_LENGTH_OR_COUNT        0x2092
#define PMERR_INV_LINE_TYPE_ATTR         0x2095
#define PMERR_INV_LINE_WIDTH_ATTR        0x2096
#define PMERR_INV_MATRIX_ELEMENT         0x209B
#define PMERR_INV_MIX_ATTR               0x20A3
#define PMERR_INV_MODIFY_PATH_MODE       0x20A6
#define PMERR_INV_OR_INCOMPAT_OPTIONS    0x20A9
#define PMERR_INV_PAGE_VIEWPORT          0x20AD
#define PMERR_INV_PATH_ID                0x20AE
#define PMERR_INV_PATTERN_ATTR           0x20B0
#define PMERR_INV_PRIMITIVE_TYPE         0x20B9
#define PMERR_INV_PS_SIZE                0x20BA
#define PMERR_INV_RECT                   0x20BD
#define PMERR_INV_REGION_CONTROL         0x20BE
#define PMERR_INV_REGION_MIX_MODE        0x20BF
#define PMERR_INV_RESERVED_FIELD         0x20C1
#define PMERR_INV_RESET_OPTIONS          0x20C2
#define PMERR_INV_SHARPNESS_PARM         0x20CD
#define PMERR_INV_TRANSFORM_TYPE         0x20D0
#define PMERR_INV_VIEWING_LIMITS         0x20D2
#define PMERR_NOT_IN_AREA                0x20DD
#define PMERR_NOT_IN_PATH                0x20E1
#define PMERR_PATH_UNKNOWN               0x20EE
#define PMERR_REGION_IS_CLIP_REGION      0x20F8
#define PMERR_UNSUPPORTED_ATTR           0x2109
#define PMERR_INV_CHAR_ALIGN_ATTR        0x2117
#define PMERR_SPL_INV_HSPL               0x4005
#define PMERR_SPL_QUEUE_NOT_FOUND        0x4024
#define PMERR_SPL_NO_DEFAULT_QUEUE       0x4028
#define SEVERITY_NOERROR                 0x0000
#define SEVERITY_WARNING                 0x0004
#define SEVERITY_ERROR                   0x0008
#define SEVERITY_SEVERE                  0x000C
#define SEVERITY_UNRECOVERABLE           0x0010

/* Standard dialog messages */
#define FDM_FILTER             0x1028
#define FDM_VALIDATE           0x1029
#define FDM_ERROR              0x102A
#define FNTM_FACENAMECHANGED   0x1032
#define FNTM_POINTSIZECHANGED  0x1033
#define FNTM_STYLECHANGED      0x1034
#define FNTM_COLORCHANGED      0x1035
#define FNTM_UPDATEPREVIEW     0x1036
#define FNTM_FILTERLIST        0x1037

/* Graphics: bundles, polygons, text boxes, resets, device */
#define ABB_COLOR                   0x0001
#define ABB_BACK_COLOR              0x0002
#define ABB_MIX_MODE                0x0004
#define ABB_BACK_MIX_MODE           0x0008
#define ABB_SET                     0x0010
#define ABB_SYMBOL                  0x0020
#define ABB_REF_POINT               0x0040
#define BA_ALTERNATE                0x0000
#define BA_INCL                     0x0000
#define BA_NOBOUNDARY               0x0000
#define BA_BOUNDARY                 0x0001
#define BA_WINDING                  0x0002
#define BA_EXCL                     0x0008
#define CAPS_FAMILY                 0x0000
#define CAPS_WIDTH                  0x0004
#define CAPS_HEIGHT                 0x0005
#define CAPS_WIDTH_IN_CHARS         0x0006
#define CAPS_HEIGHT_IN_CHARS        0x0007
#define CAPS_HORIZONTAL_RESOLUTION  0x0008
#define CAPS_VERTICAL_RESOLUTION    0x0009
#define CAPS_CHAR_WIDTH             0x000A
#define CAPS_CHAR_HEIGHT            0x000B
#define CAPS_COLORS                 0x000E
#define CAPS_COLOR_PLANES           0x000F
#define CAPS_COLOR_BITCOUNT         0x0010
#define CAPS_MOUSE_BUTTONS          0x0012
#define CAPS_BITMAP_FORMATS         0x0017
#define CAPS_PHYS_COLORS            0x0021
#define CAPS_COUNT                  0x002A
#define CBB_COLOR                   0x0001
#define CBB_BACK_COLOR              0x0002
#define CBB_MIX_MODE                0x0004
#define CBB_BACK_MIX_MODE           0x0008
#define CBB_SET                     0x0010
#define CBB_MODE                    0x0020
#define CBB_BOX                     0x0040
#define CBB_ANGLE                   0x0080
#define CBB_SHEAR                   0x0100
#define CBB_DIRECTION               0x0200
#define CBB_TEXT_ALIGN              0x0400
#define CBB_EXTRA                   0x0800
#define CBB_BREAK_EXTRA             0x1000
#define DEV_ERROR                   0x0000
#define DEV_OK                      0x0001
#define DEVESC_ERROR                (-1)
#define DEVESC_NOTIMPLEMENTED       0x0000
#define DPDM_ERROR                  (-1)
#define DTYP_USER                   0x4000
#define FWEIGHT_NORMAL              0x0005
#define FWEIGHT_BOLD                0x0007
#define FWIDTH_NORMAL               0x0005
#define GRES_ATTRS                  0x0001
#define GRES_SEGMENTS               0x0002
#define GRES_ALL                    0x0004
#define IBB_COLOR                   0x0001
#define IBB_BACK_COLOR              0x0002
#define IBB_MIX_MODE                0x0004
#define IBB_BACK_MIX_MODE           0x0008
#define LBB_COLOR                   0x0001
#define LBB_BACK_COLOR              0x0002
#define LBB_MIX_MODE                0x0004
#define LBB_BACK_MIX_MODE           0x0008
#define LBB_WIDTH                   0x0010
#define LBB_GEOM_WIDTH              0x0020
#define LBB_TYPE                    0x0040
#define LBB_END                     0x0080
#define LBB_JOIN                    0x0100
#define LINEWIDTH_ERROR             (-1)
#define LINEWIDTH_DEFAULT           0x0000
#define LINEWIDTH_NORMAL            0x00010000
#define LINEWIDTH_THICK             0x00020000
#define MBB_COLOR                   0x0001
#define MBB_BACK_COLOR              0x0002
#define MBB_MIX_MODE                0x0004
#define MBB_BACK_MIX_MODE           0x0008
#define MBB_SET                     0x0010
#define MBB_SYMBOL                  0x0020
#define MBB_BOX                     0x0040
#define OD_QUEUED                   0x0002
#define OD_DIRECT                   0x0005
#define OD_INFO                     0x0006
#define OD_MEMORY                   0x0008
#define POLYGON_ALTERNATE           0x0000
#define POLYGON_FILL                0x0000
#define POLYGON_INCL                0x0000
#define POLYGON_NOBOUNDARY          0x0000
#define POLYGON_BOUNDARY            0x0001
#define POLYGON_WINDING             0x0002
#define POLYGON_EXCL                0x0008
#define POLYGON_NOFILL              0x0010
#define PRIM_LINE                   0x0001
#define PRIM_CHAR                   0x0002
#define PRIM_MARKER                 0x0003
#define PRIM_AREA                   0x0004
#define PRIM_IMAGE                  0x0005
#define TXTBOX_TOPLEFT              0x0000
#define TXTBOX_BOTTOMLEFT           0x0001
#define TXTBOX_TOPRIGHT             0x0002
#define TXTBOX_BOTTOMRIGHT          0x0003
#define TXTBOX_CONCAT               0x0004
#define TXTBOX_COUNT                0x0005

/* Pipes and signals */
#define NPSS_EOI        0x0000
#define NPSS_RDATA      0x0001
#define NPSS_WAIT       0x0001
#define NPSS_WSPACE     0x0002
#define NPSS_CLOSE      0x0003
#define SIG_UNSETFOCUS  0x0000
#define SIG_SETFOCUS    0x0001

/* From the PM Programming Reference (values Warpine does not define) */
#define BKS_BACKPAGESBR             0x00000001
#define BKS_BACKPAGESBL             0x00000002
#define BKS_BACKPAGESTR             0x00000004
#define BKS_BACKPAGESTL             0x00000008
#define BKS_SPIRALBIND              0x00000400
#define BKS_SQUARETABS              0x00000000
#define BKS_ROUNDEDTABS             0x00000100
#define BKS_POLYGONTABS             0x00000200
#define BKS_SOLIDBIND               0x00000000
#define BKS_STATUSTEXTLEFT          0x00000000
#define BKS_TABTEXTLEFT             0x00000000
#define BOOKERR_INVALID_PARAMETERS  (-1)
#define CCS_SINGLESEL               0x00000004
#define CCS_VERIFYPOINTERS          0x00000010
#define CFA_LEFT                    0x00000001
#define CFA_TOP                     0x00000008
#define CFA_VCENTER                 0x00000010
#define CFA_BOTTOM                  0x00000020
#define CFI_OWNERFREE               0x0001
#define CFI_OWNERDISPLAY            0x0002
#define CFI_HANDLE                  0x0200
#define CFI_POINTER                 0x0400
#define CMA_INVALIDATE              0x0002
#define CMA_REPOSITION              0x0002
#define CMA_NOREPOSITION            0x0001
#define CMA_TEXTCHANGED             0x0004
#define CMA_ERASE                   0x0008
#define CM_MODE2                    2
#define DBM_NORMAL                  0x0000
#define DBM_HALFTONE                0x0002
#define DBM_IMAGEATTRS              0x0008
#define DDECTXT_CASESENSITIVE       1
#define DDEFMT_TEXT                 1
#define DDE_FACK                    0x0001
#define DDE_FBUSY                   0x0002
#define DDE_FNODATA                 0x0004
#define DDE_FACKREQ                 0x0008
#define DDE_FRESPONSE               0x0010
#define DDE_NOTPROCESSED            0x0020
#define DDE_FRESERVED               0x00C0
#define DDE_FAPPSTATUS              0xFF00
#define DEVESC_QUERYESCSUPPORT      0
#define DEVESC_STARTDOC             8150
#define DEVESC_ENDDOC               8151
#define DEVESC_ABORTDOC             8153
#define DEVESC_NEWFRAME             16300
#define DPDM_POSTJOBPROP            0
#define DPDM_CHANGEPROP             1
#define DPDM_QUERYJOBPROP           2
#define DP_NORMAL                   0
#define DRG_ICON                    0x00000001
#define DRG_BITMAP                  0x00000002
#define DRG_POLYGON                 0x00000004
#define DRG_STRETCH                 0x00000008
#define DRG_TRANSPARENT             0x00000010
#define DRG_CLOSED                  0x00000020
#define DTYP_LONG                   (25)
#define DTYP_ULONG                  (26)
#define DTYP_SHORT                  (23)
#define DTYP_USHORT                 (24)
#define DTYP_BYTE                   (13)
#define DTYP_UCHAR                  (22)
#define DTYP_CTL_ARRAY              (1)
#define DTYP_CTL_OFFSET             (2)
#define DTYP_CTL_LENGTH             (3)
#define DTYP_CTL_PARRAY             (-1)
#define HM_RESOURCEID               0
#define HM_PANELNAME                1
#define LS_NOADJUSTPOS              0x0004
#define LS_HORZSCROLL               0x0008
#define PATSYM_DENSE2               2
#define PATSYM_DENSE3               3
#define PATSYM_DENSE4               4
#define PATSYM_DENSE5               5
#define PATSYM_DENSE6               6
#define PATSYM_DENSE7               7
#define PU_MOUSEBUTTON1             0x0040
#define PU_MOUSEBUTTON2             0x0080
#define PU_MOUSEBUTTON3             0x0100
#define PU_NONE                     0x0000
#define QWS_USER                    0
#define RUM_IN                      1
#define RUM_OUT                     2
#define RUM_INOUT                   3
#define SEI_BREAKPOINT              0x8000
#define SEI_NOBEEP                  0x4000
#define SEI_NOPROMPT                0x2000
#define SEI_DBGRSRVD                0x1000
#define SEI_STACKTRACE              0x0001
#define SEI_REGISTERS               0x0002
#define SEI_ARGCOUNT                0x0004
#define SEI_DOSERROR                0x0008
#define SHE_VISIBLE                 0
#define SHE_INVISIBLE               1
#define SWL_INVISIBLE               1
#define SWL_GRAYED                  2
#define SWL_NOTJUMPABLE             1
#define SPL_PR_QUEUE                0x00000001
#define SPL_PR_DIRECT_DEVICE        0x00000002
#define SPL_PR_QUEUED_DEVICE        0x00000004
#define SPL_PR_LOCAL_ONLY           0x00000100
#define CF_TEXT                     1
#define CF_BITMAP                   2
#define CF_DSPTEXT                  3
#define CF_DSPBITMAP                4
#define CF_METAFILE                 5
#define CF_DSPMETAFILE              6
#define CF_PALETTE                  9

/* ── Containers ─────────────────────────────────────────────────────── */
typedef struct _TREEITEMDESC {
    HBITMAP  hbmExpanded;
    HBITMAP  hbmCollapsed;
    HPOINTER hptrExpanded;
    HPOINTER hptrCollapsed;
} TREEITEMDESC, *PTREEITEMDESC;

typedef struct _MINIRECORDCORE {
    ULONG                   cb;
    ULONG                   flRecordAttr;
    POINTL                  ptlIcon;
    struct _MINIRECORDCORE *preccNextRecord;
    PSZ                     pszIcon;
    HPOINTER                hptrIcon;
} MINIRECORDCORE, *PMINIRECORDCORE;

typedef struct _RECORDCORE {
    ULONG               cb;
    ULONG               flRecordAttr;
    POINTL              ptlIcon;
    struct _RECORDCORE *preccNextRecord;
    PSZ                 pszIcon;
    HPOINTER            hptrIcon;
    HPOINTER            hptrMiniIcon;
    HBITMAP             hbmBitmap;
    HBITMAP             hbmMiniBitmap;
    PTREEITEMDESC       pTreeItemDesc;
    PSZ                 pszText;
    PSZ                 pszName;
    PSZ                 pszTree;
} RECORDCORE, *PRECORDCORE;

typedef struct _FIELDINFO {
    ULONG              cb;
    ULONG              flData;
    ULONG              flTitle;
    PVOID              pTitleData;
    ULONG              offStruct;
    PVOID              pUserData;
    struct _FIELDINFO *pNextFieldInfo;
    ULONG              cxWidth;
} FIELDINFO, *PFIELDINFO;

typedef struct _FIELDINFOINSERT {
    ULONG      cb;
    PFIELDINFO pFieldInfoOrder;
    ULONG      fInvalidateFieldInfo;
    ULONG      cFieldInfoInsert;
} FIELDINFOINSERT, *PFIELDINFOINSERT;

typedef struct _RECORDINSERT {
    ULONG       cb;
    PRECORDCORE pRecordOrder;
    PRECORDCORE pRecordParent;
    ULONG       fInvalidateRecord;
    ULONG       zOrder;
    ULONG       cRecordsInsert;
} RECORDINSERT, *PRECORDINSERT;

typedef struct _CNRINFO {
    ULONG       cb;
    PVOID       pSortRecord;
    PFIELDINFO  pFieldInfoLast;
    PFIELDINFO  pFieldInfoObject;
    PSZ         pszCnrTitle;
    ULONG       flWindowAttr;
    POINTL      ptlOrigin;
    ULONG       cDelta;
    ULONG       cRecords;
    SIZEL       slBitmapOrIcon;
    SIZEL       slTreeBitmapOrIcon;
    HBITMAP     hbmExpanded;
    HBITMAP     hbmCollapsed;
    HPOINTER    hptrExpanded;
    HPOINTER    hptrCollapsed;
    LONG        cyLineSpacing;
    LONG        cxTreeIndent;
    LONG        cxTreeLine;
    ULONG       cFields;
    LONG        xVertSplitbar;
} CNRINFO, *PCNRINFO;

typedef struct _CDATE { UCHAR day; UCHAR month; USHORT year; } CDATE, *PCDATE;
typedef struct _CTIME { UCHAR hours; UCHAR minutes; UCHAR seconds; UCHAR ucReserved; } CTIME, *PCTIME;

typedef struct _NOTIFYRECORDEMPHASIS {
    HWND        hwndCnr;
    PRECORDCORE pRecord;
    ULONG       fEmphasisMask;
} NOTIFYRECORDEMPHASIS, *PNOTIFYRECORDEMPHASIS;

typedef struct _NOTIFYRECORDENTER {
    HWND        hwndCnr;
    ULONG       fKey;
    PRECORDCORE pRecord;
} NOTIFYRECORDENTER, *PNOTIFYRECORDENTER;

typedef struct _NOTIFYSCROLL {
    HWND  hwndCnr;
    LONG  lScrollInc;
    ULONG fScroll;
} NOTIFYSCROLL, *PNOTIFYSCROLL;

typedef struct _NOTIFYDELTA {
    HWND  hwndCnr;
    ULONG fDelta;
} NOTIFYDELTA, *PNOTIFYDELTA;

typedef struct _CNREDITDATA {
    ULONG       cb;
    HWND        hwndCnr;
    PRECORDCORE pRecord;
    PFIELDINFO  pFieldInfo;
    PSZ        *ppszText;
    ULONG       cbText;
    ULONG       id;
} CNREDITDATA, *PCNREDITDATA;

typedef struct _QUERYRECFROMRECT {
    ULONG cb;
    RECTL rect;
    ULONG fsSearch;
} QUERYRECFROMRECT, *PQUERYRECFROMRECT;

typedef struct _QUERYRECORDRECT {
    ULONG       cb;
    PRECORDCORE pRecord;
    ULONG       fRightSplitWindow;
    ULONG       fsExtent;
} QUERYRECORDRECT, *PQUERYRECORDRECT;

typedef struct _SEARCHSTRING {
    ULONG cb;
    PSZ   pszSearch;
    ULONG fsPrefix;
    ULONG fsCaseSensitive;
    ULONG usView;
} SEARCHSTRING, *PSEARCHSTRING;

typedef struct _TREEMOVE {
    PRECORDCORE preccMove;
    PRECORDCORE preccNewParent;
    PRECORDCORE pRecordOrder;
    BOOL        flMoveSiblings;
} TREEMOVE, *PTREEMOVE;

typedef struct _GRIDSQUARE {
    ULONG ulNumber;
    ULONG ulState;
    RECTL rctlSquare;
} GRIDSQUARE, *PGRIDSQUARE;

typedef struct _GRIDINFO {
    ULONG       cb;
    SHORT       cxGrid;
    SHORT       cyGrid;
    SHORT       sGridRows;
    SHORT       sGridCols;
    LONG        cGridSquares;
    PGRIDSQUARE pGrid;
} GRIDINFO, *PGRIDINFO;

/* ── Notebooks ──────────────────────────────────────────────────────── */
typedef struct _PAGESELECTNOTIFY {
    HWND  hwndBook;
    ULONG ulPageIdCur;
    ULONG ulPageIdNew;
} PAGESELECTNOTIFY, *PPAGESELECTNOTIFY;

typedef struct _DELETENOTIFY {
    HWND    hwndBook;
    HWND    hwndPage;
    ULONG   ulAppPageData;
    HBITMAP hbmTab;
} DELETENOTIFY, *PDELETENOTIFY;

typedef struct _BOOKTEXT {
    PSZ   pString;
    ULONG textLen;
} BOOKTEXT, *PBOOKTEXT;

typedef struct _BOOKPAGEINFO {
    ULONG        cb;
    ULONG        fl;
    BOOL         bLoadDlg;
    ULONG        ulPageData;
    HWND         hwndPage;
    PFN          pfnPageDlgProc;
    ULONG        idPageDlg;
    HMODULE      hmodPageDlg;
    PVOID        pPageDlgCreateParams;
    PDLGTEMPLATE pdlgtPage;
    ULONG        cbStatusLine;
    PSZ          pszStatusLine;
    HBITMAP      hbmMajorTab;
    HBITMAP      hbmMinorTab;
    ULONG        cbMajorTab;
    PSZ          pszMajorTab;
    ULONG        cbMinorTab;
    PSZ          pszMinorTab;
    PVOID        pBidiInfo;
} BOOKPAGEINFO, *PBOOKPAGEINFO;

typedef struct _NOTEBOOKBUTTON {
    PSZ     pszText;
    ULONG   idButton;
    LHANDLE hImage;
    LONG    flStyle;
} NOTEBOOKBUTTON, *PNOTEBOOKBUTTON;

/* ── Sliders, value sets, scroll bars ───────────────────────────────── */
typedef struct _SLDCDATA {
    ULONG  cbSize;
    USHORT usScale1Increments;
    USHORT usScale1Spacing;
    USHORT usScale2Increments;
    USHORT usScale2Spacing;
} SLDCDATA, *PSLDCDATA;

typedef struct _CSBITMAPDATA {
    HBITMAP hbmLeftUp;
    HBITMAP hbmLeftDown;
    HBITMAP hbmRightUp;
    HBITMAP hbmRightDown;
} CSBITMAPDATA, *PCSBITMAPDATA;

typedef struct _VSCDATA {
    ULONG  cbSize;
    USHORT usRowCount;
    USHORT usColumnCount;
} VSCDATA, *PVSCDATA;

typedef struct _VSTEXT {
    PSZ   pszItemText;
    ULONG ulBufLen;
} VSTEXT, *PVSTEXT;

typedef struct _SBCDATA {
    USHORT cb;
    USHORT sHilite;
    SHORT  posFirst;
    SHORT  posLast;
    SHORT  posThumb;
    SHORT  cVisible;
    SHORT  cTotal;
} SBCDATA, *PSBCDATA;

/* ── Owner drawing and user buttons ─────────────────────────────────── */
typedef struct _OWNERITEM {
    HWND  hwnd;
    HPS   hps;
    ULONG fsState;
    ULONG fsAttribute;
    ULONG fsStateOld;
    ULONG fsAttributeOld;
    RECTL rclItem;
    LONG  idItem;
    ULONG hItem;
} OWNERITEM, *POWNERITEM;

typedef struct _USERBUTTON {
    HWND  hwnd;
    HPS   hps;
    ULONG fsState;
    ULONG fsStateOld;
} USERBUTTON, *PUSERBUTTON;

/* Button and dialog helpers. */
#define WinCheckButton(hwndDlg, id, usCheckState) \
    ((ULONG)WinSendDlgItemMsg(hwndDlg, id, BM_SETCHECK, MPFROMSHORT(usCheckState), (MPARAM)NULL))
#define WinQueryButtonCheckstate(hwndDlg, id) \
    ((ULONG)WinSendDlgItemMsg(hwndDlg, id, BM_QUERYCHECK, (MPARAM)NULL, (MPARAM)NULL))
#define WinEnableControl(hwndDlg, id, fEnable) WinEnableWindow(WinWindowFromID(hwndDlg, id), fEnable)
#define WinIsControlEnabled(hwndDlg, id) ((BOOL)WinIsWindowEnabled(WinWindowFromID(hwndDlg, id)))

/* More values programs use */
#define CLR_NOINDEX                (-254)
#define CBM_SHOWLIST               0x0170
#define CBM_HILITE                 0x0171
#define CBM_ISLISTSHOWING          0x0172
#define DID_FILENAME_TXT           0x0101
#define DID_FILENAME_ED            0x0102
#define DID_DRIVE_TXT              0x0103
#define DID_DRIVE_CB               0x0104
#define DID_FILTER_TXT             0x0105
#define DID_FILTER_CB              0x0106
#define DID_DIRECTORY_TXT          0x0107
#define DID_DIRECTORY_LB           0x0108
#define DID_FILES_TXT              0x0109
#define DID_FILES_LB               0x010A
#define DID_HELP_PB                0x010B
#define DID_APPLY_PB               0x010C
#define DID_NAME                   0x012D
#define DID_STYLE                  0x012E
#define DID_SIZE                   0x0131
#define DID_SAMPLE                 0x0132
#define DID_OUTLINE                0x0133
#define DID_UNDERSCORE             0x0134
#define DID_STRIKEOUT              0x0135
#define DID_HELP_BUTTON            0x0136
#define DID_APPLY_BUTTON           0x0137
#define DID_RESET_BUTTON           0x0138
#define DID_NAME_PREFIX            0x0139
#define DID_STYLE_PREFIX           0x013A
#define DID_SIZE_PREFIX            0x013B
#define DID_SAMPLE_GROUPBOX        0x013C
#define DID_EMPHASIS_GROUPBOX      0x013D
#define PU_POSITIONONITEM          0x0001
#define PU_HCONSTRAIN              0x0002
#define PU_VCONSTRAIN              0x0004
#define PU_MOUSEBUTTON1DOWN        0x0008
#define PU_SELECTITEM              0x0020
#define PU_KEYBOARD                0x0200
#define PS_UNITS                   0x00FC
#define PS_FORMAT                  0x0F00
#define PS_TYPE                    0x1000
#define PS_MODE                    0x2000
#define PS_ASSOCIATE               0x4000
#define PS_NORESET                 0x8000
#define QWL_MIN                    (-11)
#define SLDERR_INVALID_PARAMETERS  (-1)
#define PM_MODEL_1X                0
#define PM_MODEL_2X                1
#define NERR_BASE                  2100
#define NERR_QNotFound             (NERR_BASE + 50)

/* What a container's owner draws (WM_DRAWITEM's OWNERITEM.hItem). */
typedef struct _CNRDRAWITEMINFO {
    PRECORDCORE pRecord;
    PFIELDINFO  pFieldInfo;
} CNRDRAWITEMINFO, *PCNRDRAWITEMINFO;

/* What CN_DRAGOVER / CN_DROP / CN_DRAGLEAVE give (mp2). */
typedef struct _CNRDRAGINFO {
    PDRAGINFO   pDragInfo;
    PRECORDCORE pRecord;
} CNRDRAGINFO, *PCNRDRAGINFO;

typedef struct _CNRDRAGINIT {
    HWND        hwndCnr;
    PRECORDCORE pRecord;
    LONG        x;
    LONG        y;
    LONG        cx;
    LONG        cy;
} CNRDRAGINIT, *PCNRDRAGINIT;

/* What a value set's VN_INITDRAG / VN_DRAGOVER / VN_DROP give (mp2). */
typedef struct _VSDRAGINIT {
    HWND   hwnd;
    LONG   x;
    LONG   y;
    LONG   cx;
    LONG   cy;
    USHORT usRow;
    USHORT usColumn;
} VSDRAGINIT, *PVSDRAGINIT;

typedef struct _VSDRAGINFO {
    PDRAGINFO pDragInfo;
    USHORT    usRow;
    USHORT    usColumn;
} VSDRAGINFO, *PVSDRAGINFO;

/* ── DDE ────────────────────────────────────────────────────────────── */
typedef struct _CONVCONTEXT {
    ULONG cb;
    ULONG fsContext;
    ULONG idCountry;
    ULONG usCodepage;
    ULONG usLangID;
    ULONG usSubLangID;
} CONVCONTEXT, *PCONVCONTEXT;

typedef struct _DDEINIT {
    ULONG cb;
    PSZ   pszAppName;
    PSZ   pszTopic;
    ULONG offConvContext;
} DDEINIT, *PDDEINIT;

/* Followed by the item name and the data, at the offsets it gives. */
typedef struct _DDESTRUCT {
    ULONG  cbData;
    USHORT fsStatus;
    USHORT usFormat;
    USHORT offszItemName;
    USHORT offabData;
} DDESTRUCT, *PDDESTRUCT;

#define DDES_PSZITEMNAME(pddes) (((PSZ)(pddes)) + ((PDDESTRUCT)(pddes))->offszItemName)
#define DDES_PABDATA(pddes)     (((PBYTE)(pddes)) + ((PDDESTRUCT)(pddes))->offabData)
#define DDEI_PCONVCONTEXT(pddei) ((PCONVCONTEXT)((PBYTE)(pddei) + ((PDDEINIT)(pddei))->offConvContext))

BOOL    APIENTRY WinDdeInitiate(HWND hwndClient, PCSZ pszAppName, PCSZ pszTopicName, PCONVCONTEXT pcctxt);
MRESULT APIENTRY WinDdeRespond(HWND hwndClient, HWND hwndServer, PCSZ pszAppName, PCSZ pszTopicName,
                               PCONVCONTEXT pcctxt);
BOOL    APIENTRY WinDdePostMsg(HWND hwndTo, HWND hwndFrom, ULONG wm, PDDESTRUCT pddest, ULONG flOptions);

/* ── Tracking rectangles, hooks ─────────────────────────────────────── */
typedef struct _TRACKINFO {
    LONG   cxBorder;
    LONG   cyBorder;
    LONG   cxGrid;
    LONG   cyGrid;
    LONG   cxKeyboard;
    LONG   cyKeyboard;
    RECTL  rclTrack;
    RECTL  rclBoundary;
    POINTL ptlMinTrackSize;
    POINTL ptlMaxTrackSize;
    ULONG  fs;
} TRACKINFO, *PTRACKINFO;

BOOL    APIENTRY WinTrackRect(HWND hwnd, HPS hps, PTRACKINFO pti);

/* What a HK_SENDMSG hook receives. */
typedef struct _SMHSTRUCT {
    MPARAM mp2;
    MPARAM mp1;
    ULONG  msg;
    HWND   hwnd;
    ULONG  model;
} SMHSTRUCT, *PSMHSTRUCT;

/* ── Attribute bundles (GpiSetAttrs / GpiQueryAttrs) ────────────────── */
typedef PVOID PBUNDLE;

typedef struct _LINEBUNDLE {
    LONG   lColor;
    LONG   lBackColor;
    USHORT usMixMode;
    USHORT usBackMixMode;
    FIXED  fxWidth;
    LONG   lGeomWidth;
    USHORT usType;
    USHORT usEnd;
    USHORT usJoin;
    USHORT usReserved;
} LINEBUNDLE, *PLINEBUNDLE;

typedef struct _CHARBUNDLE {
    LONG   lColor;
    LONG   lBackColor;
    USHORT usMixMode;
    USHORT usBackMixMode;
    USHORT usSet;
    USHORT usPrecision;
    SIZEF  sizfxCell;
    POINTL ptlAngle;
    POINTL ptlShear;
    USHORT usDirection;
    USHORT usTextAlign;
    FIXED  fxExtra;
    FIXED  fxBreakExtra;
} CHARBUNDLE, *PCHARBUNDLE;

typedef struct _AREABUNDLE {
    LONG   lColor;
    LONG   lBackColor;
    USHORT usMixMode;
    USHORT usBackMixMode;
    USHORT usSet;
    USHORT usSymbol;
    POINTL ptlRefPoint;
} AREABUNDLE, *PAREABUNDLE;

typedef struct _MARKERBUNDLE {
    LONG   lColor;
    LONG   lBackColor;
    USHORT usMixMode;
    USHORT usBackMixMode;
    USHORT usSet;
    USHORT usSymbol;
    SIZEF  sizfxCell;
} MARKERBUNDLE, *PMARKERBUNDLE;

typedef struct _IMAGEBUNDLE {
    LONG   lColor;
    LONG   lBackColor;
    USHORT usMixMode;
    USHORT usBackMixMode;
} IMAGEBUNDLE, *PIMAGEBUNDLE;

LONG    APIENTRY GpiQueryAttrs(HPS hps, LONG lPrimType, ULONG flAttrMask, PBUNDLE ppbunAttrs);
BOOL    APIENTRY GpiSetAttrs(HPS hps, LONG lPrimType, ULONG flAttrMask, ULONG flDefMask, PBUNDLE ppbunAttrs);

/* ── Devices and the spooler's queries ──────────────────────────────── */
typedef PVOID PDRIVDATA;
typedef ULONG SPLERR;
LONG    APIENTRY DevEscape(HDC hdc, LONG lCode, LONG lInCount, PBYTE pbInData, PLONG plOutCount, PBYTE pbOutData);
LONG    APIENTRY DevPostDeviceModes(HAB hab, PDRIVDATA pdrivDriverData, PCSZ pszDriverName, PCSZ pszDeviceName,
                                    PCSZ pszName, ULONG flOptions);
SPLERR  APIENTRY SplEnumQueue(PCSZ pszComputerName, ULONG ulLevel, PVOID pBuf, ULONG cbBuf, PULONG pcReturned,
                              PULONG pcTotal, PULONG pcbNeeded, PVOID pReserved);
SPLERR  APIENTRY SplEnumDevice(PCSZ pszComputerName, ULONG ulLevel, PVOID pBuf, ULONG cbBuf, PULONG pcReturned,
                               PULONG pcTotal, PULONG pcbNeeded, PVOID pReserved);
SPLERR  APIENTRY SplEnumPrinter(PCSZ pszComputerName, ULONG uLevel, ULONG flType, PVOID pBuf, ULONG cbbuf,
                                PULONG pcReturned, PULONG pcTotal, PULONG pcbNeeded, PVOID pReserved);
SPLERR  APIENTRY SplEnumDriver(PCSZ pszComputerName, ULONG ulLevel, PVOID pBuf, ULONG cbBuf, PULONG pcReturned,
                               PULONG pcTotal, PULONG pcbNeeded, PVOID pReserved);
SPLERR  APIENTRY SplEnumPort(PCSZ pszComputerName, ULONG ulLevel, PVOID pBuf, ULONG cbBuf, PULONG pcReturned,
                             PULONG pcTotal, PULONG pcbNeeded, PVOID pReserved);
SPLERR  APIENTRY SplEnumQueueProcessor(PCSZ pszComputerName, ULONG ulLevel, PVOID pBuf, ULONG cbBuf,
                                       PULONG pcReturned, PULONG pcTotal, PULONG pcbNeeded, PVOID pReserved);
SPLERR  APIENTRY SplQueryQueue(PCSZ pszComputerName, PCSZ pszQueueName, ULONG ulLevel, PVOID pBuf, ULONG cbBuf,
                               PULONG pcbNeeded);
SPLERR  APIENTRY SplQueryDevice(PCSZ pszComputerName, PCSZ pszPrintDeviceName, ULONG ulLevel, PVOID pBuf,
                                ULONG cbBuf, PULONG pcbNeeded);

/* ── More window-manager calls ──────────────────────────────────────── */
typedef LHANDLE HLIB;
typedef HATOMTABLE HATOMTBL;
#define MAKEINTATOM(a)       ((PCH)MAKEULONG(a, 0xFFFF))
#define ERRORIDERROR(errid)  (LOUSHORT(errid))
#define ERRORIDSEV(errid)    (HIUSHORT(errid))
#define FIELDOFFSET(type, field) ((SHORT)(ULONG)&(((type *)0)->field))

ERRORID APIENTRY WinSetErrorInfo(ERRORID errorid, ULONG fsOptions, ...);
HLIB    APIENTRY WinLoadLibrary(HAB hab, PCSZ libname);
BOOL    APIENTRY WinDeleteLibrary(HAB hab, HLIB libhandle);
PFNWP   APIENTRY WinLoadProcedure(HAB hab, HLIB libhandle, PSZ procname);
BOOL    APIENTRY WinDeleteProcedure(HAB hab, PFNWP wndproc);
BOOL    APIENTRY WinMakePoints(HAB hab, PPOINTL pwpt, ULONG cwpt);
BOOL    APIENTRY WinMakeRect(HAB hab, PRECTL pwrc);
BOOL    APIENTRY WinRegisterUserDatatype(HAB hab, LONG datatype, LONG count, PLONG types);
BOOL    APIENTRY WinRegisterUserMsg(HAB hab, ULONG msgid, LONG datatype1, LONG dir1, LONG datatype2, LONG dir2,
                                    LONG datatyper);
BOOL    APIENTRY WinSetSysColors(HWND hwndDesktop, ULONG flOptions, ULONG flFormat, LONG clrFirst, ULONG cclr,
                                 PLONG pclr);
ULONG   APIENTRY WinCpTranslateChar(HAB hab, ULONG cpSrc, UCHAR chSrc, ULONG cpDst);
BOOL    APIENTRY WinStoreWindowPos(PCSZ pszAppName, PCSZ pszKeyName, HWND hwnd);
BOOL    APIENTRY WinRestoreWindowPos(PCSZ pszAppName, PCSZ pszKeyName, HWND hwnd);

/* Workplace Shell objects (PMWP; ADR-0129, ADR-0130): a handle names a
 * stored object; WinQueryObject finds the one an object ID ("<NAME>")
 * names, NULLHANDLE when none does. WinCreateObject and WinSetObjectData
 * queue the object's setup until a program that has its class asks for it
 * (wpclsQueryObject). The CO_ values are Warpine's. */
typedef LHANDLE HOBJECT;
#define CO_FAILIFEXISTS    0
#define CO_REPLACEIFEXISTS 1
#define CO_UPDATEIFEXISTS  2
HOBJECT APIENTRY WinQueryObject(PCSZ pszObjectID);
HOBJECT APIENTRY WinCreateObject(PCSZ pszClassName, PCSZ pszTitle, PCSZ pszSetupString, PCSZ pszLocation, ULONG ulFlags);
BOOL    APIENTRY WinDestroyObject(HOBJECT hObject);
BOOL    APIENTRY WinSetObjectData(HOBJECT hObject, PCSZ pszSetupString);

#pragma pack(pop)

#endif /* WARPINE_OS2_PMCTL_H */
