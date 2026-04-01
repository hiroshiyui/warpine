// SPDX-License-Identifier: GPL-3.0-only

use super::constants::*;
use super::api_trace;
use super::api_registry;
use super::{ApiResult, ApiCallRecord};
use super::mutex_ext::MutexExt;
use super::vm_backend::VcpuBackend;
use tracing::{debug, warn};

impl super::Loader {
    pub(crate) fn handle_api_call_ex(&self, vcpu: &mut dyn VcpuBackend, vcpu_id: u32, ordinal: u32) -> ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let ret_addr = self.guest_read::<u32>(esp as u32).expect("Stack read OOB");

        let api_name = api_trace::ordinal_to_name(ordinal);
        let api_mod  = api_trace::module_for_ordinal(ordinal);
        let _span = tracing::debug_span!(
            "api",
            vcpu   = vcpu_id,
            module = api_mod,
            ord    = ordinal,
            name   = api_name,
            eip    = ret_addr,
            esp    = esp as u32,
        ).entered();

        // Pre-read 10 stack args once (covers DosDevIOCtl's 9-arg maximum).
        // args[i] = guest u32 at ESP + 4 + i*4  (_System calling convention).
        let args: [u32; 10] = std::array::from_fn(|i| {
            self.guest_read::<u32>((esp + 4 + i as u64 * 4) as u32).unwrap_or(0)
        });

        // Always compute the formatted call string — used both for the strace
        // debug log and for the ring buffer (which must be populated regardless
        // of log level so crash dumps include the full recent call history).
        let call_str = api_trace::format_call(api_name, ordinal, &args,
            &|ptr| self.read_guest_string(ptr));

        if tracing::enabled!(tracing::Level::DEBUG) {
            debug!("{}", call_str);
        }

        let result = if let Some(entry) = api_registry::find(ordinal) {
            (entry.handler)(self, vcpu, vcpu_id, args)
        } else {
            // Registry covers DOSCALLS, QUECALLS, NLS, and MDM.
            // Remaining subsystems use their own sub-dispatchers.
            match ordinal {
                o if (PMWIN_BASE..PMGPI_BASE).contains(&o) => {
                    self.handle_pmwin_call(vcpu, vcpu_id, o - PMWIN_BASE)
                }
                o if (PMGPI_BASE..KBDCALLS_BASE).contains(&o) => {
                    self.handle_pmgpi_call(vcpu, vcpu_id, o - PMGPI_BASE)
                }
                o if (KBDCALLS_BASE..VIOCALLS_BASE).contains(&o) => {
                    self.handle_kbdcalls(vcpu, vcpu_id, o - KBDCALLS_BASE)
                }
                o if (VIOCALLS_BASE..SESMGR_BASE).contains(&o) => {
                    self.handle_viocalls(vcpu, vcpu_id, o - VIOCALLS_BASE)
                }
                _ => {
                    warn!("Unknown API ordinal {} ({}) on VCPU {}", ordinal, api_name, vcpu_id);
                    ApiResult::Normal(0)
                }
            }
        };

        let ret_val = match &result {
            ApiResult::Normal(v)               => { debug!(ret = v, "return"); *v }
            ApiResult::Callback { wnd_proc, .. } => { debug!(wnd_proc, "callback"); 0 }
            ApiResult::WmCreateCallback { wnd_proc, .. } => { debug!(wnd_proc, "wm_create_callback"); 0 }
            ApiResult::CallGuest { addr, .. }  => { debug!(addr, "call_guest"); 0 }
            ApiResult::ExceptionDispatch { handler_addr, .. } => {
                debug!(handler_addr, "exception_dispatch"); 0
            }
        };

        // Push to ring buffer unconditionally (populated even in release/info builds).
        self.shared.api_ring.lock_or_recover().push(ApiCallRecord {
            ordinal,
            module: api_mod,
            name: api_name,
            call_str,
            ret_val,
            seq: 0, // overwritten by ApiRingBuffer::push()
        });

        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Loader, ApiResult};
    use super::super::vm_backend::mock::MockVcpu;
    use super::super::constants::{KBDCALLS_BASE, VIOCALLS_BASE};

    /// Set up a minimal stack: fake return address at ESP, args at ESP+4, +8, …
    fn setup_stack(loader: &Loader, vcpu: &mut MockVcpu, esp: u32, args: &[u32]) {
        vcpu.regs.rsp = esp as u64;
        loader.guest_write::<u32>(esp, 0xCAFEBABE).unwrap(); // fake return address
        for (i, &arg) in args.iter().enumerate() {
            loader.guest_write::<u32>(esp + 4 + i as u32 * 4, arg).unwrap();
        }
    }

    // ── KBDCALLS routing ─────────────────────────────────────────────────────

    #[test]
    fn test_dispatch_routes_kbdcalls_kbd_get_status() {
        // KbdGetStatus = KBDCALLS_BASE + 10
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp = 0x1000u32;
        let p_status = 0x2000u32;
        // Pascal: ESP+4=hkbd=0, ESP+8=pStatus
        setup_stack(&loader, &mut vcpu, esp, &[0, p_status]);
        let result = loader.handle_api_call_ex(&mut vcpu, 0, KBDCALLS_BASE + 10);
        assert!(matches!(result, ApiResult::Normal(0)));
        // KBDINFO.cb field should be 10
        assert_eq!(loader.guest_read::<u16>(p_status), Some(10));
    }

    // ── VIOCALLS routing ─────────────────────────────────────────────────────

    #[test]
    fn test_dispatch_routes_viocalls_vio_get_cur_type() {
        // VioGetCurType = VIOCALLS_BASE + 27
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp = 0x1000u32;
        let p_cur_data = 0x2000u32;
        // Pascal: ESP+4=hvio=0, ESP+8=pCurData
        setup_stack(&loader, &mut vcpu, esp, &[0, p_cur_data]);
        let result = loader.handle_api_call_ex(&mut vcpu, 0, VIOCALLS_BASE + 27);
        assert!(matches!(result, ApiResult::Normal(0)));
    }

    // ── DOSCALLS registry routing ────────────────────────────────────────────

    #[test]
    fn test_dispatch_routes_doscalls_query_h_type() {
        // DosQueryHType = ordinal 224
        // _System: args[0]=hfile, args[1]=pType, args[2]=pAttr at ESP+4, +8, +12
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp = 0x1000u32;
        let p_type = 0x2000u32;
        let p_attr = 0x2004u32;
        setup_stack(&loader, &mut vcpu, esp, &[0, p_type, p_attr]); // hfile=0 (stdin)
        let result = loader.handle_api_call_ex(&mut vcpu, 0, 224);
        assert!(matches!(result, ApiResult::Normal(0)));
        assert_eq!(loader.guest_read::<u32>(p_type), Some(1)); // stdin = char device
    }

    // ── Unknown ordinal ──────────────────────────────────────────────────────

    #[test]
    fn test_dispatch_unknown_ordinal_returns_ok() {
        let loader = Loader::new_mock();
        let mut vcpu = MockVcpu::new();
        let esp = 0x1000u32;
        setup_stack(&loader, &mut vcpu, esp, &[]);
        // 50000 is outside all known subsystem ranges
        let result = loader.handle_api_call_ex(&mut vcpu, 0, 50000);
        assert!(matches!(result, ApiResult::Normal(0)));
    }
}
