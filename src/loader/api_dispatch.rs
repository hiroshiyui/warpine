// SPDX-License-Identifier: GPL-3.0-only

use super::constants::*;
use super::api_trace;
use super::api_registry;
use super::ApiResult;
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

        match &result {
            ApiResult::Normal(v) => debug!(ret = v, "return"),
            ApiResult::Callback { wnd_proc, .. } => debug!(wnd_proc, "callback"),
        }
        result
    }
}
