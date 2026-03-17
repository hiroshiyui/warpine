// SPDX-License-Identifier: GPL-3.0-only

use super::constants::*;
use super::ApiResult;
use kvm_ioctls::VcpuFd;
use log::{debug, warn};

impl super::Loader {
    pub(crate) fn handle_api_call_ex(&self, vcpu: &mut VcpuFd, vcpu_id: u32, ordinal: u32) -> ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| -> u32 { self.guest_read::<u32>((esp + off) as u32).expect("Stack read OOB") };

        debug!("  [VCPU {}] API Call: Ordinal {} (ReturnAddr=0x{:08X})", vcpu_id, ordinal, read_stack(0));

        if ordinal < 1024 {
            // DOSCALLS
            let res = match ordinal {
                256 => self.dos_set_file_ptr(read_stack(4), read_stack(8) as i32, read_stack(12), read_stack(16)),
                257 => self.dos_close(read_stack(4)),
                259 => self.dos_delete(read_stack(4)),
                271 => self.dos_move(read_stack(4), read_stack(8)),
                226 => self.dos_delete_dir(read_stack(4)),
                270 => self.dos_create_dir(read_stack(4)),
                273 => self.dos_open(read_stack(4), read_stack(8), read_stack(12), read_stack(24), read_stack(28)),
                281 => self.dos_read(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                282 => self.dos_write(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                229 => self.dos_sleep(read_stack(4)),
                311 => self.dos_create_thread(vcpu_id, read_stack(4), read_stack(8), read_stack(12), read_stack(20)),
                234 => {
                    // DosExit: signal clean shutdown instead of process::exit
                    let _action = read_stack(4);
                    let result = read_stack(8);
                    self.shared.exit_code.store(result as i32, std::sync::atomic::Ordering::Relaxed);
                    self.shared.exit_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                    return ApiResult::Normal(0); // won't be used; run_vcpu will exit
                },
                239 => self.dos_create_pipe(read_stack(4), read_stack(8), read_stack(12)),
                312 => self.dos_get_info_blocks(vcpu, read_stack(4), read_stack(8)),
                283 => self.dos_exec_pgm(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)),
                280 => self.dos_wait_child(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                235 => self.dos_kill_process(read_stack(4), read_stack(8)), // DosKillProcess
                264 => self.dos_find_first(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)),
                265 => self.dos_find_next(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                263 => self.dos_find_close(read_stack(4)),
                223 => self.dos_query_path_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                // Directory management
                255 => self.dos_set_current_dir(read_stack(4)),
                274 => self.dos_query_current_dir(read_stack(4), read_stack(8), read_stack(12)),
                275 => self.dos_query_current_disk(read_stack(4), read_stack(8)),
                220 => self.dos_set_default_disk(read_stack(4)),
                278 => self.dos_query_fs_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                299 => self.dos_alloc_mem(read_stack(4), read_stack(8)),
                304 => self.dos_free_mem(read_stack(4)),
                324 => self.dos_create_event_sem(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                326 => self.dos_close_event_sem(read_stack(4)),
                328 => self.dos_post_event_sem(read_stack(4)),
                329 => self.dos_wait_event_sem(read_stack(4), read_stack(8)),
                331 => self.dos_create_mutex_sem(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                333 => self.dos_close_mutex_sem(read_stack(4)),
                334 => self.dos_request_mutex_sem(vcpu_id, read_stack(4), read_stack(8)),
                335 => self.dos_release_mutex_sem(vcpu_id, read_stack(4)),
                337 => self.dos_create_mux_wait_sem(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                339 => self.dos_close_mux_wait_sem(read_stack(4)),
                340 => self.dos_wait_mux_wait_sem(vcpu_id, read_stack(4), read_stack(8), read_stack(12)),
                323 => self.dos_query_app_type(read_stack(4), read_stack(8)),
                342 => { debug!("DosDeleteMuxWaitSem stub"); 0 }, // DosDeleteMuxWaitSem
                349 => self.dos_wait_thread(vcpu_id, read_stack(4)),
                352 => self.dos_get_resource(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                353 => self.dos_free_resource(read_stack(4)),
                572 => self.dos_query_resource_size(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                // Step 1: Critical init stubs
                212 => self.dos_error(read_stack(4)),
                209 => self.dos_set_max_fh(read_stack(4)),
                286 => self.dos_beep(read_stack(4), read_stack(8)),
                354 => self.dos_set_exception_handler(read_stack(4)),
                355 => self.dos_unset_exception_handler(read_stack(4)),
                356 => { debug!("DosRaiseException stub"); 0 }, // DosRaiseException
                418 => self.dos_acknowledge_signal_exception(read_stack(4)),
                368 => self.dos_query_sys_state(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)), // DosQuerySysState
                378 => self.dos_set_signal_exception_focus(read_stack(4)), // DosSetSignalExceptionFocus
                380 => self.dos_enter_must_complete(read_stack(4)),
                381 => self.dos_exit_must_complete(read_stack(4)),
                // Step 2: Shared memory
                300 => self.dos_alloc_shared_mem(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                301 => self.dos_get_named_shared_mem(read_stack(4), read_stack(8), read_stack(12)),
                302 => self.dos_get_shared_mem(read_stack(4), read_stack(8)),
                305 => self.dos_set_mem(read_stack(4), read_stack(8), read_stack(12)),
                306 => self.dos_query_mem(read_stack(4), read_stack(8), read_stack(12)),
                // Step 3: Codepage and country info
                291 => self.dos_query_cp(read_stack(4), read_stack(8), read_stack(12)),
                289 => self.dos_set_process_cp(read_stack(4)),
                397 => self.dos_query_ctry_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)), // DosQueryCtryInfo
                // Step 4: Module loading stubs
                318 => self.dos_load_module(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                322 => self.dos_free_module(read_stack(4)),
                319 => self.dos_query_module_handle(read_stack(4), read_stack(8)),
                321 => self.dos_query_proc_addr(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                317 => { debug!("DosDebug stub"); 87 }, // DosDebug (not implemented)
                // Step 5: File metadata APIs
                258 => self.dos_copy(read_stack(4), read_stack(8), read_stack(12)),
                261 => self.dos_edit_name(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                279 => self.dos_query_file_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                267 => { debug!("DOS16REQUESTVDD stub"); 0 }, // DOS16REQUESTVDD
                219 => self.dos_set_path_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                276 => self.dos_query_fh_state(read_stack(4), read_stack(8)),
                277 => self.dos_query_fs_attach(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)), // DosQueryFSAttach
                // Ordinals 297/298 do not exist in DOSCALLS — removed (were phantom duplicates)
                // Step 6: Device I/O stubs
                284 => self.dos_dev_ioctl(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28), read_stack(32), read_stack(36)),
                231 => self.dos_dev_config(read_stack(4), read_stack(8)),  // NOTE: may conflict with DosSetDateTime
                // Step 7: Semaphore extensions
                325 => self.dos_open_event_sem(read_stack(4), read_stack(8)),
                332 => self.dos_open_mutex_sem(read_stack(4), read_stack(8)),
                // Step 8: Named pipe stubs
                230 => self.dos_get_date_time(read_stack(4)),
                348 => self.dos_query_sys_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)),
                // Additional APIs needed by 4OS2
                382 => self.dos_set_rel_max_fh(read_stack(4), read_stack(8)),
                272 => self.dos_set_file_size(read_stack(4), read_stack(8)),
                260 => self.dos_dup_handle(read_stack(4), read_stack(8)),
                254 => self.dos_reset_buffer(read_stack(4)),
                210 => self.dos_set_verify(read_stack(4)),
                225 => self.dos_query_verify(read_stack(4)),
                292 => self.dos_set_date_time(read_stack(4)),
                218 => self.dos_set_file_info(read_stack(4), read_stack(8), read_stack(12), read_stack(16)), // DosSetFileInfo
                285 => { debug!("DosFSCtl stub"); 0 }, // DosFSCtl - stub
                357 => { debug!("DosUnwindException stub"); 0 }, // DosUnwindException - stub
                372 => self.dos_enum_attribute(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28)),
                428 => self.dos_set_file_locks(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)),
                639 => self.dos_protect_set_file_locks(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24)),
                415 => { debug!("DosShutdown stub"); 0 }, // DosShutdown - stub
                425 => self.dos_flat_to_sel(read_stack(4)), // DosFlatToSel
                426 => self.dos_sel_to_flat(read_stack(4)), // DosSelToFlat
                241 => { debug!("DosConnectNPipe stub"); 0 }, // DosConnectNPipe - stub
                243 => { debug!("DosCreateNPipe stub"); 0 }, // DosCreateNPipe - stub
                250 => { debug!("DosSetNPHState stub"); 0 }, // DosSetNPHState - stub
                221 => self.dos_set_fh_state(read_stack(4), read_stack(8)), // alias for old ordinal
                224 => self.dos_query_h_type(read_stack(4), read_stack(8), read_stack(12)), // alias
                110 => { debug!("DosForceDelete stub (ord 110)"); self.dos_delete(read_stack(4)) },
                // 16-bit thunks
                8 => self.dos_get_info_seg(read_stack(4), read_stack(8)),
                75 => self.dos_query_file_mode_16(read_stack(4), read_stack(8)),
                84 => self.dos_set_file_mode(read_stack(4), read_stack(8)),
                _ => { warn!("Warning: Unknown API Ordinal {} on VCPU {}", ordinal, vcpu_id); 0 }
            };
            ApiResult::Normal(res)
        } else if ordinal < 2048 {
            // QUECALLS
            // QUECALLS ordinals from doc/os2_ordinals.md
            let res = match ordinal - 1024 {
                16 => self.dos_create_queue(read_stack(4), read_stack(8), read_stack(12)),   // DosCreateQueue
                15 => self.dos_open_queue(read_stack(4), read_stack(8), read_stack(12)),     // DosOpenQueue
                14 => self.dos_write_queue(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20)), // DosWriteQueue
                9  => self.dos_read_queue(read_stack(4), read_stack(8), read_stack(12), read_stack(16), read_stack(20), read_stack(24), read_stack(28), read_stack(32)), // DosReadQueue
                11 => self.dos_close_queue(read_stack(4)),                                   // DosCloseQueue
                10 => { self.dos_purge_queue(read_stack(4)); 0 },                           // DosPurgeQueue
                12 => self.dos_query_queue(read_stack(4), read_stack(8)),                    // DosQueryQueue
                _ => { warn!("Warning: Unknown QUECALLS Ordinal {} on VCPU {}", ordinal - 1024, vcpu_id); 0 }
            };
            ApiResult::Normal(res)
        } else if ordinal < PMGPI_BASE {
            // PMWIN
            self.handle_pmwin_call(vcpu, vcpu_id, ordinal - PMWIN_BASE)
        } else if ordinal < KBDCALLS_BASE {
            // PMGPI
            self.handle_pmgpi_call(vcpu, vcpu_id, ordinal - PMGPI_BASE)
        } else if ordinal < VIOCALLS_BASE {
            // KBDCALLS
            self.handle_kbdcalls(vcpu, vcpu_id, ordinal - KBDCALLS_BASE)
        } else if ordinal < SESMGR_BASE {
            // VIOCALLS
            self.handle_viocalls(vcpu, vcpu_id, ordinal - VIOCALLS_BASE)
        } else if ordinal < NLS_BASE {
            // SESMGR
            let sesmgr_ord = ordinal - SESMGR_BASE;
            warn!("SESMGR stub: ordinal {} on VCPU {}", sesmgr_ord, vcpu_id);
            ApiResult::Normal(0)
        } else if ordinal < MSG_BASE {
            // NLS (National Language Support)
            let nls_ord = ordinal - NLS_BASE;
            let res = match nls_ord {
                5 => {
                    // NLS ordinal 5 — _System convention
                    // On real OS/2, this is DosQueryCp for small cb (codepage query).
                    // But the CRT wrapper calls it with cb=44 (sizeof COUNTRYINFO)
                    // to retrieve full country information. When cb >= 44, the
                    // layout appears to be: (cb, pcc, pci_output)
                    let cb = read_stack(4);
                    let arg2 = read_stack(8);
                    let arg3 = read_stack(12);
                    debug!("NLS ordinal 5: cb={} arg2=0x{:08X} arg3=0x{:08X}", cb, arg2, arg3);
                    if cb >= 44 {
                        // Return full COUNTRYINFO to arg3 (the output buffer)
                        self.dos_query_ctry_info(cb, arg2, arg3, 0)
                    } else {
                        // Standard DosQueryCp: (cb, pCP, pcb)
                        self.dos_query_cp(cb, arg2, arg3)
                    }
                }
                6 => {
                    // DosQueryCtryInfo(cb, pcc, pci, pcb_actual) — _System convention
                    let cb = read_stack(4);
                    let pcc = read_stack(8);
                    let pci = read_stack(12);
                    let pcb = read_stack(16);
                    debug!("NLS DosQueryCtryInfo: cb={} pcc=0x{:08X} pci=0x{:08X} pcb=0x{:08X}", cb, pcc, pci, pcb);
                    self.dos_query_ctry_info(cb, pcc, pci, pcb)
                }
                7 => {
                    // DosMapCase(cb, pcc, pch) — _System convention
                    let cb = read_stack(4);
                    let _pcc = read_stack(8);
                    let pch = read_stack(12);
                    debug!("NLS DosMapCase(cb={}, pch=0x{:08X})", cb, pch);
                    for i in 0..cb {
                        if let Some(ch) = self.guest_read::<u8>(pch + i) {
                            if ch >= b'a' && ch <= b'z' {
                                let _ = self.guest_write::<u8>(pch + i, ch - 32);
                            }
                        }
                    }
                    0
                }
                8 => {
                    // DosGetDBCSEv(cb, pcc, pch) — _System convention
                    let cb = read_stack(4);
                    let _pcc = read_stack(8);
                    let pch = read_stack(12);
                    debug!("NLS DosGetDBCSEv(cb={}, pch=0x{:08X})", cb, pch);
                    if pch != 0 && cb >= 2 {
                        let _ = self.guest_write::<u16>(pch, 0);
                    }
                    0
                }
                _ => {
                    warn!("NLS stub: ordinal {} on VCPU {}", nls_ord, vcpu_id);
                    0
                }
            };
            ApiResult::Normal(res)
        } else if ordinal < STUB_AREA_SIZE {
            // MSG
            let msg_ord = ordinal - MSG_BASE;
            warn!("MSG stub: ordinal {} on VCPU {}", msg_ord, vcpu_id);
            ApiResult::Normal(0)
        } else {
            warn!("Warning: Unknown API Base Ordinal {} on VCPU {}", ordinal, vcpu_id);
            ApiResult::Normal(0)
        }
    }
}
