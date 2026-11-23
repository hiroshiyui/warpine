/* SPDX-License-Identifier: GPL-3.0-or-later */
/* os2/errors.h - OS/2 error codes (INCL_DOSERRORS), as the Control
 * Program Programming Reference lists them for each call. `gen_api
 * check-headers` checks every code here that Warpine also defines
 * against Warpine's own value. */
#ifndef WARPINE_OS2_ERRORS_H
#define WARPINE_OS2_ERRORS_H

#ifndef NO_ERROR
#  define NO_ERROR                    0
#endif
#define ERROR_INVALID_FUNCTION        1
#define ERROR_FILE_NOT_FOUND          2
#define ERROR_PATH_NOT_FOUND          3
#define ERROR_TOO_MANY_OPEN_FILES     4
#define ERROR_ACCESS_DENIED           5
#define ERROR_INVALID_HANDLE          6
#define ERROR_NOT_ENOUGH_MEMORY       8
#define ERROR_BAD_FORMAT              11
#define ERROR_INVALID_ACCESS          12
#define ERROR_INVALID_DATA            13
#define ERROR_INVALID_DRIVE           15
#define ERROR_CURRENT_DIRECTORY       16
#define ERROR_NOT_SAME_DEVICE         17
#define ERROR_NO_MORE_FILES           18
#define ERROR_WRITE_PROTECT           19
#define ERROR_BAD_LENGTH              24
#define ERROR_SHARING_VIOLATION       32
#define ERROR_LOCK_VIOLATION          33
#define ERROR_HANDLE_EOF              38
#define ERROR_NOT_SUPPORTED           50
#define ERROR_FILE_EXISTS             80
#define ERROR_CANNOT_MAKE             82
#define ERROR_INVALID_PARAMETER       87
#define ERROR_NO_PROC_SLOTS           89
#define ERROR_NOT_FROZEN              90
#define ERROR_INTERRUPT               95
#define ERROR_TOO_MANY_SEMAPHORES     100
#define ERROR_SEM_OWNER_DIED          105
#define ERROR_BROKEN_PIPE             109
#define ERROR_OPEN_FAILED             110
#define ERROR_BUFFER_OVERFLOW         111
#define ERROR_DISK_FULL               112
#define ERROR_NO_MORE_SEARCH_HANDLES  113
#define ERROR_CALL_NOT_IMPLEMENTED    120
#define ERROR_SEM_TIMEOUT             121
#define ERROR_INSUFFICIENT_BUFFER     122
#define ERROR_INVALID_NAME            123
#define ERROR_INVALID_LEVEL           124
#define ERROR_MOD_NOT_FOUND           126
#define ERROR_PROC_NOT_FOUND          127
#define ERROR_WAIT_NO_CHILDREN        128
#define ERROR_CHILD_NOT_COMPLETE      129
#define ERROR_NEGATIVE_SEEK           131
#define ERROR_SEEK_ON_DEVICE          132
#define ERROR_DIR_NOT_EMPTY           145
#define ERROR_SIGNAL_REFUSED          156
#define ERROR_MAX_THRDS_REACHED       164
#define ERROR_ALREADY_EXISTS          183
#define ERROR_SEM_NOT_FOUND           187
#define ERROR_ENVVAR_NOT_FOUND        203
#define ERROR_NO_SIGNAL_SENT          205
#define ERROR_FILENAME_EXCED_RANGE    206
#define ERROR_INVALID_SIGNAL_NUMBER   209
#define ERROR_NESTING_NOT_ALLOWED     215
#define ERROR_ZOMBIE_PROCESS          217
#define ERROR_BAD_PIPE                230
#define ERROR_PIPE_BUSY               231
#define ERROR_NO_DATA                 232
#define ERROR_PIPE_NOT_CONNECTED      233
#define ERROR_MORE_DATA               234
#define ERROR_DUPLICATE_HANDLE        284
#define ERROR_DUPLICATE_NAME          285
#define ERROR_EMPTY_MUXWAIT           286
#define ERROR_NOT_OWNER               288
#define ERROR_PARAM_TOO_SMALL         289
#define ERROR_TOO_MANY_HANDLES        290
#define ERROR_TOO_MANY_OPENS          291
#define ERROR_WRONG_TYPE              292
#define ERROR_THREAD_NOT_TERMINATED   294
#define ERROR_INIT_ROUTINE_FAILED     295
#define ERROR_TOO_MANY_POSTS          298
#define ERROR_ALREADY_POSTED          299
#define ERROR_ALREADY_RESET           300
#define ERROR_SEM_BUSY                301
#define ERROR_INVALID_PROCID          303
#define ERROR_NOT_DESCENDANT          305
#define ERROR_INVALID_THREADID        309
#define ERROR_DOSSUB_SHRINK           310
#define ERROR_DOSSUB_NOMEM            311
#define ERROR_DOSSUB_OVERLAP          312
#define ERROR_DOSSUB_BADSIZE          313
#define ERROR_DOSSUB_BADFLAG          314
#define ERROR_MR_MSG_TOO_LONG         316
#define ERROR_MR_MID_NOT_FOUND        317
#define ERROR_MR_UN_ACC_MSGF          318
#define ERROR_MR_INV_MSGF_FORMAT      319
#define ERROR_MR_INV_IVCOUNT          320
#define ERROR_TS_SEMHANDLE            323
#define ERROR_TS_NOTIMER              324
#define ERROR_TS_HANDLE               326
#define ERROR_TS_DATETIME             327
#define ERROR_QUE_ELEMENT_NOT_EXIST   333
#define ERROR_QUE_INVALID_HANDLE      337
#define ERROR_QUE_EMPTY               342
#define ERROR_VIO_ROW                 358
#define ERROR_VIO_COL                 359
#define ERROR_KBD_NO_DEVICE           374
#define ERROR_MOUSE_NO_DEVICE         385
#define ERROR_MOUSE_INV_HANDLE        386
#define ERROR_MOUSE_INV_PARMS         387
#define ERROR_MOUSE_INV_MASK          392
#define ERROR_MOUSE_SMG_ONLY          412
#define ERROR_MOUSE_INVALID_ASCIIZ    413
#define ERROR_MOUSE_INVALID_MASK      414
#define ERROR_MOUSE_REGISTER          415
#define ERROR_MOUSE_DEREGISTER        416
#define ERROR_NLS_TABLE_TRUNCATED     399
#define ERROR_VIO_INVALID_PARMS       421
#define ERROR_INVALID_CODE_PAGE       472
#define ERROR_CRITSEC_OVERFLOW        484
#define ERROR_CRITSEC_UNDERFLOW       485
#define ERROR_INVALID_ADDRESS         487
#define ERROR_DOSSUB_CORRUPTED        532
#define ERROR_TIMEOUT                 640
#define ERROR_NESTING_TOO_DEEP        650

#endif /* WARPINE_OS2_ERRORS_H */
