// SPDX-License-Identifier: GPL-3.0-or-later
//
// The texts `mciGetErrorString` (MDM 3) returns for the MCI error codes.
//
// The codes are those of the toolkit's <meerror.h> (Open Watcom's copy). IBM's
// message texts are not in any public source, so each text is the code's own
// name written out in words (Warpine's wording, doc/unverified.md).

#![forbid(unsafe_code)]

/// `(code - MCIERR_BASE, text)` of every MCI error code, in code order.
pub const MCI_ERROR_TEXTS: &[(u32, &str)] = &[
    (  1, "Invalid device ID"),  // MCIERR_INVALID_DEVICE_ID
    (  2, "No master"),  // MCIERR_NO_MASTER
    (  3, "Unrecognized keyword"),  // MCIERR_UNRECOGNIZED_KEYWORD
    (  4, "Master conflict"),  // MCIERR_MASTER_CONFLICT
    (  5, "Unrecognized command"),  // MCIERR_UNRECOGNIZED_COMMAND
    (  6, "Hardware error"),  // MCIERR_HARDWARE
    (  7, "Invalid device name"),  // MCIERR_INVALID_DEVICE_NAME
    (  8, "Out of memory"),  // MCIERR_OUT_OF_MEMORY
    (  9, "Device open"),  // MCIERR_DEVICE_OPEN
    ( 10, "Cannot load driver"),  // MCIERR_CANNOT_LOAD_DRIVER
    ( 11, "Missing command string"),  // MCIERR_MISSING_COMMAND_STRING
    ( 12, "Param overflow"),  // MCIERR_PARAM_OVERFLOW
    ( 13, "Missing string argument"),  // MCIERR_MISSING_STRING_ARGUMENT
    ( 14, "Bad integer"),  // MCIERR_BAD_INTEGER
    ( 15, "Parser internal error"),  // MCIERR_PARSER_INTERNAL
    ( 16, "Driver internal error"),  // MCIERR_DRIVER_INTERNAL
    ( 17, "Missing parameter"),  // MCIERR_MISSING_PARAMETER
    ( 18, "Unsupported function"),  // MCIERR_UNSUPPORTED_FUNCTION
    ( 19, "File not found"),  // MCIERR_FILE_NOT_FOUND
    ( 20, "Device not ready"),  // MCIERR_DEVICE_NOT_READY
    ( 21, "Internal error"),  // MCIERR_INTERNAL
    ( 22, "Driver error"),  // MCIERR_DRIVER
    ( 23, "Cannot use all"),  // MCIERR_CANNOT_USE_ALL
    ( 24, "Multiple errors"),  // MCIERR_MULTIPLE
    ( 25, "Extension not found"),  // MCIERR_EXTENSION_NOT_FOUND
    ( 26, "Out of range"),  // MCIERR_OUTOFRANGE
    ( 27, "Cannot add alias"),  // MCIERR_CANNOT_ADD_ALIAS
    ( 28, "Flags not compatible"),  // MCIERR_FLAGS_NOT_COMPATIBLE
    ( 29, "Cannot use nounload"),  // MCIERR_CANNOT_USE_NOUNLOAD
    ( 30, "File not saved"),  // MCIERR_FILE_NOT_SAVED
    ( 31, "Device type required"),  // MCIERR_DEVICE_TYPE_REQUIRED
    ( 32, "Device locked"),  // MCIERR_DEVICE_LOCKED
    ( 33, "Duplicate alias"),  // MCIERR_DUPLICATE_ALIAS
    ( 34, "Instance inactive"),  // MCIERR_INSTANCE_INACTIVE
    ( 35, "Command table error"),  // MCIERR_COMMAND_TABLE
    ( 37, "INI file locked"),  // MCIERR_INI_FILE_LOCKED
    ( 40, "No audio support"),  // MCIERR_NO_AUDIO_SUPPORT
    ( 41, "Not in PM session"),  // MCIERR_NOT_IN_PM_SESSION
    ( 42, "Duplicate keyword"),  // MCIERR_DUPLICATE_KEYWORD
    ( 43, "Command string overflow"),  // MCIERR_COMMAND_STRING_OVERFLOW
    ( 44, "Driver proc not found"),  // MCIERR_DRIVER_PROC_NOT_FOUND
    ( 45, "Invalid device type"),  // MCIERR_INVALID_DEVICE_TYPE
    ( 46, "Invalid device ordinal"),  // MCIERR_INVALID_DEVICE_ORDINAL
    ( 47, "Headphones not set"),  // MCIERR_HEADPHONES_NOT_SET
    ( 48, "Speakers not set"),  // MCIERR_SPEAKERS_NOT_SET
    ( 49, "Sound not set"),  // MCIERR_SOUND_NOT_SET
    ( 50, "Invalid buffer"),  // MCIERR_INVALID_BUFFER
    ( 51, "Invalid media type"),  // MCIERR_INVALID_MEDIA_TYPE
    ( 52, "Invalid connector index"),  // MCIERR_INVALID_CONNECTOR_INDEX
    ( 53, "No connection"),  // MCIERR_NO_CONNECTION
    ( 54, "Invalid flag"),  // MCIERR_INVALID_FLAG
    ( 55, "Cannot load DSP mod"),  // MCIERR_CANNOT_LOAD_DSP_MOD
    ( 56, "Already connected"),  // MCIERR_ALREADY_CONNECTED
    ( 57, "Invalid callback handle"),  // MCIERR_INVALID_CALLBACK_HANDLE
    ( 58, "Driver not found"),  // MCIERR_DRIVER_NOT_FOUND
    ( 59, "Duplicate driver"),  // MCIERR_DUPLICATE_DRIVER
    ( 60, "INI file error"),  // MCIERR_INI_FILE
    ( 61, "Invalid group ID"),  // MCIERR_INVALID_GROUP_ID
    ( 62, "ID already in group"),  // MCIERR_ID_ALREADY_IN_GROUP
    ( 63, "Media changed"),  // MCIERR_MEDIA_CHANGED
    ( 64, "Missing flag"),  // MCIERR_MISSING_FLAG
    ( 65, "Unsupported flag"),  // MCIERR_UNSUPPORTED_FLAG
    ( 66, "Driver not loaded"),  // MCIERR_DRIVER_NOT_LOADED
    ( 67, "Invalid mode"),  // MCIERR_INVALID_MODE
    ( 68, "Invalid item flag"),  // MCIERR_INVALID_ITEM_FLAG
    ( 69, "Invalid time format flag"),  // MCIERR_INVALID_TIME_FORMAT_FLAG
    ( 70, "Speed format flag"),  // MCIERR_SPEED_FORMAT_FLAG
    ( 71, "Invalid audio flag"),  // MCIERR_INVALID_AUDIO_FLAG
    ( 72, "No default device"),  // MCIERR_NODEFAULT_DEVICE
    ( 73, "Duplicate extension"),  // MCIERR_DUPLICATE_EXTENSION
    ( 74, "File attribute"),  // MCIERR_FILE_ATTRIBUTE
    ( 75, "Duplicate cue point"),  // MCIERR_DUPLICATE_CUEPOINT
    ( 76, "Invalid cue point"),  // MCIERR_INVALID_CUEPOINT
    ( 77, "Cue point limit reached"),  // MCIERR_CUEPOINT_LIMIT_REACHED
    ( 78, "Missing item"),  // MCIERR_MISSING_ITEM
    ( 79, "Missing time format"),  // MCIERR_MISSING_TIME_FORMAT
    ( 80, "Missing speed format"),  // MCIERR_MISSING_SPEED_FORMAT
    ( 81, "Invalid connector type"),  // MCIERR_INVALID_CONNECTOR_TYPE
    ( 82, "Target device full"),  // MCIERR_TARGET_DEVICE_FULL
    ( 83, "Unsupported connection type"),  // MCIERR_UNSUPPORTED_CONN_TYPE
    ( 84, "Cannot modify connector"),  // MCIERR_CANNOT_MODIFY_CONNECTOR
    ( 85, "Record aborted"),  // MCIERR_RECORD_ABORTED
    ( 86, "Group command"),  // MCIERR_GROUP_COMMAND
    ( 87, "Device not found"),  // MCIERR_DEVICE_NOT_FOUND
    ( 88, "Resource not available"),  // MCIERR_RESOURCE_NOT_AVAILABLE
    ( 89, "Invalid IO proc"),  // MCIERR_INVALID_IO_PROC
    ( 90, "Wave outputs in use"),  // MCIERR_WAVE_OUTPUTSINUSE
    ( 91, "Wave set output in use"),  // MCIERR_WAVE_SETOUTPUTINUSE
    ( 92, "Wave inputs in use"),  // MCIERR_WAVE_INPUTSINUSE
    ( 93, "Wave set input in use"),  // MCIERR_WAVE_SETINPUTINUSE
    ( 94, "Wave output unspecified"),  // MCIERR_WAVE_OUTPUTUNSPECIFIED
    ( 95, "Wave input unspecified"),  // MCIERR_WAVE_INPUTUNSPECIFIED
    ( 96, "Wave outputs unsuitable"),  // MCIERR_WAVE_OUTPUTSUNSUITABLE
    ( 97, "Wave set output unsuitable"),  // MCIERR_WAVE_SETOUTPUTUNSUITABLE
    ( 98, "Wave inputs unsuitable"),  // MCIERR_WAVE_INPUTSUNSUITABLE
    ( 99, "Wave set input unsuitable"),  // MCIERR_WAVE_SETINPUTUNSUITABLE
    (100, "Sequencer division incompatible"),  // MCIERR_SEQ_DIV_INCOMPATIBLE
    (101, "Sequencer port inuse"),  // MCIERR_SEQ_PORT_INUSE
    (102, "Sequencer port nonexistent"),  // MCIERR_SEQ_PORT_NONEXISTENT
    (103, "Sequencer port map no device"),  // MCIERR_SEQ_PORT_MAPNODEVICE
    (104, "Sequencer port error"),  // MCIERR_SEQ_PORT_MISCERROR
    (105, "Sequencer timer error"),  // MCIERR_SEQ_TIMER
    (106, "Videodisc command cancelled"),  // MCIERR_VDP_COMMANDCANCELLED
    (107, "Videodisc command failure"),  // MCIERR_VDP_COMMANDFAILURE
    (108, "Videodisc not spun up"),  // MCIERR_VDP_NOTSPUNUP
    (109, "Videodisc no chapter"),  // MCIERR_VDP_NOCHAPTER
    (110, "Videodisc no side"),  // MCIERR_VDP_NOSIDE
    (111, "Videodisc no size"),  // MCIERR_VDP_NOSIZE
    (112, "Videodisc invalid time format"),  // MCIERR_VDP_INVALID_TIMEFORMAT
    (114, "Clipboard error"),  // MCIERR_CLIPBOARD_ERROR
    (115, "Cannot convert"),  // MCIERR_CANNOT_CONVERT
    (116, "Cannot redo"),  // MCIERR_CANNOT_REDO
    (117, "Cannot undo"),  // MCIERR_CANNOT_UNDO
    (118, "Clipboard empty"),  // MCIERR_CLIPBOARD_EMPTY
    (119, "Invalid work path"),  // MCIERR_INVALID_WORKPATH
    (120, "Indeterminate length"),  // MCIERR_INDETERMINATE_LENGTH
    (121, "Duplicate EA"),  // MCIERR_DUPLICATE_EA
    (122, "Invalid connection"),  // MCIERR_INVALID_CONNECTION
    (123, "Channel off"),  // MCIERR_CHANNEL_OFF
    (124, "Cannot change channel"),  // MCIERR_CANNOT_CHANGE_CHANNEL
    (125, "File I/O error"),  // MCIERR_FILE_IO
    (126, "System file error"),  // MCIERR_SYSTEM_FILE
    (127, "Display resolution"),  // MCIERR_DISPLAY_RESOLUTION
    (128, "No async play active"),  // MCIERR_NO_ASYNC_PLAY_ACTIVE
    (129, "Unsupported format tag"),  // MCIERR_UNSUPP_FORMAT_TAG
    (130, "Unsupported samples per second"),  // MCIERR_UNSUPP_SAMPLESPERSEC
    (131, "Unsupported bits per sample"),  // MCIERR_UNSUPP_BITSPERSAMPLE
    (132, "Unsupported channels"),  // MCIERR_UNSUPP_CHANNELS
    (133, "Unsupported format mode"),  // MCIERR_UNSUPP_FORMAT_MODE
    (134, "No device driver"),  // MCIERR_NO_DEVICE_DRIVER
    (135, "Codec not supported"),  // MCIERR_CODEC_NOT_SUPPORTED
    (136, "Tuner no HW"),  // MCIERR_TUNER_NO_HW
    (137, "Tuner no AFC"),  // MCIERR_TUNER_NO_AFC
    (138, "Tuner AFC on"),  // MCIERR_TUNER_AFC_ON
    (139, "Tuner channel skipped"),  // MCIERR_TUNER_CHANNEL_SKIPPED
    (140, "Tuner channel too low"),  // MCIERR_TUNER_CHANNEL_TOO_LOW
    (141, "Tuner channel too high"),  // MCIERR_TUNER_CHANNEL_TOO_HIGH
    (142, "Audio channel out of range"),  // MCIERR_AUD_CHANNEL_OUTOFRANGE
    (143, "Tuner invalid region"),  // MCIERR_TUNER_INVALID_REGION
    (144, "Signal invalid"),  // MCIERR_SIGNAL_INVALID
    (145, "Tuner mode"),  // MCIERR_TUNER_MODE
    (146, "Tuner region not set"),  // MCIERR_TUNER_REGION_NOT_SET
    (147, "Tuner channel not set"),  // MCIERR_TUNER_CHANNEL_NOT_SET
    (148, "Unsupported class"),  // MCIERR_UNSUPP_CLASS
    (149, "Unsupported attribute"),  // MCIERR_UNSUPPORTED_ATTRIBUTE
];
