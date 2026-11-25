/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * wpsint.h - what the SDK's Workplace Shell classes share inside libwps
 * (ADR-0129); not for programs.
 *
 * - The awake objects that have a handle, by handle (wpclsQueryObject
 *   answers from it before it looks in the user profile).
 * - A WPObject's saved values (wpSaveString / Long / Data) as one record,
 *   and back: a storage class keeps the record.
 * - WPAbstract's records in the user profile (Warpine's own format):
 *   application WRP_WPS_APP_OBJECTS, key the handle in eight hex digits,
 *   data a record (WRP_WPS_RECORD_MAGIC, version, class name, saved
 *   values); WRP_WPS_APP_IDS maps an object ID (key) to its handle (four
 *   bytes, little-endian), which WinQueryObject reads; WRP_WPS_APP_PENDING
 *   holds, by handle, setup strings WinCreateObject / WinSetObjectData
 *   queued (ADR-0130): a record without saved values is an object no
 *   process has made yet; WRP_WPS_APP_HANDLES / WRP_WPS_KEY_NEXT holds the
 *   next handle to give. src/loader/wps_setup.rs names the same.
 */
#ifndef WRP_WPSINT_H
#define WRP_WPSINT_H

#define WRP_WPS_APP_OBJECTS  "WrpWPS:Objects"
#define WRP_WPS_APP_IDS      "WrpWPS:ObjectIDs"
#define WRP_WPS_APP_PENDING  "WrpWPS:Pending"
#define WRP_WPS_APP_HANDLES  "WrpWPS:Handles"
#define WRP_WPS_KEY_NEXT     "Next"
#define WRP_WPS_FIRST_HANDLE 0x00020001UL
#define WRP_WPS_RECORD_MAGIC 0x4F505757UL   /* "WWPO" */
#define WRP_WPS_RECORD_VERSION 1UL
/* The largest record read back. */
#define WRP_WPS_MAX_RECORD   ( 1UL << 20 )

/* The awake objects with a handle. */
BOOL      wrp_wps_register( HOBJECT h, WPObject *obj );
void      wrp_wps_unregister( WPObject *obj );
WPObject *wrp_wps_lookup( HOBJECT h );

/* Mark an object initialized (wpclsNew's last step; an awakened one). */
void      wrp_wps_set_initialized( WPObject *obj );

/* A stored object awakened from the user profile (wpabstract.c); NULL
 * when there is none, or its class is not registered. */
WPObject *wrp_wps_awaken( HOBJECT h );

/* What WinSetObjectData queued for an awake stored object, applied
 * (wpSetup) and saved; nothing for another object. */
void      wrp_wps_apply_pending( WPObject *obj );

/* A record of the object (WRP_WPS_RECORD_MAGIC, version, class name, its
 * saved values), SOMMalloc'd, its size at *size; NULL when it cannot be
 * made or is larger than WRP_WPS_MAX_RECORD. */
unsigned char *wrp_wps_record_make( WPObject *obj, ULONG *size );
/* A record's class name (into name, WP_MAX_TITLE bytes) and its saved
 * values' part; FALSE when the record is not well formed. */
BOOL wrp_wps_record_parse( const unsigned char *rec, ULONG size, char *name,
                           const unsigned char **part, ULONG *part_len );

/* The bytes the object's saved values take as a record's part; written
 * to buf only when cap is enough. */
ULONG wrp_wps_store_write( WPObject *obj, unsigned char *buf, ULONG cap );
/* The saved values from a record's part (len bytes) put back; FALSE
 * when the part is not well formed. */
BOOL  wrp_wps_store_read( WPObject *obj, const unsigned char *buf, ULONG len );

#endif
