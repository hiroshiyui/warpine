// SPDX-License-Identifier: GPL-3.0-only
//
// OS/2 Presentation Manager PMGPI API handler methods.

use super::vm_backend::VcpuBackend;
use log::{debug, warn};

use super::mutex_ext::MutexExt;
use super::ApiResult;
use crate::gui::GUIMessage;

impl super::Loader {
    pub(crate) fn handle_pmgpi_call(&self, vcpu: &mut dyn VcpuBackend, vcpu_id: u32, ordinal: u32) -> ApiResult {
        let regs = vcpu.get_regs().unwrap();
        let esp = regs.rsp;
        let read_stack = |off: u64| -> u32 { self.guest_read::<u32>((esp + off) as u32).expect("Stack read OOB") };

        match ordinal {
            369 => {
                // GpiCreatePS(HAB hab, HDC hdc, PSIZEL pszl, ULONG flOptions)
                let _hab = read_stack(4);
                let _hdc = read_stack(8);
                let _pszl = read_stack(12);
                let _opts = read_stack(16);
                let hps = self.shared.window_mgr.lock_or_recover().create_ps(0);
                debug!("  [VCPU {}] GpiCreatePS -> HPS {}", vcpu_id, hps);
                ApiResult::Normal(hps)
            }
            379 => {
                // GpiDestroyPS(HPS hps)
                let hps = read_stack(4);
                self.shared.window_mgr.lock_or_recover().ps_map.remove(&hps);
                ApiResult::Normal(1)
            }
            517 => {
                // GpiSetColor(HPS hps, LONG lColor)
                let hps = read_stack(4);
                let color = read_stack(8);
                let mapped = self.map_color(color);
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) {
                    ps.color = mapped;
                }
                ApiResult::Normal(1)
            }
            404 => {
                // GpiMove(HPS hps, PPOINTL pptl)
                let hps = read_stack(4);
                let pptl = read_stack(8);
                let (x, y) = (
                    self.guest_read::<i32>(pptl).unwrap_or(0),
                    self.guest_read::<i32>(pptl + 4).unwrap_or(0),
                );
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get_mut(&hps) {
                    ps.current_pos = (x, y);
                }
                ApiResult::Normal(1)
            }
            356 => {
                // GpiBox(HPS hps, LONG lControl, PPOINTL pptl, LONG lHRound, LONG lVRound)
                let hps = read_stack(4);
                let control = read_stack(8);
                let pptl = read_stack(12);
                let _h_round = read_stack(16);
                let _v_round = read_stack(20);
                let (x2, y2) = (
                    self.guest_read::<i32>(pptl).unwrap_or(0),
                    self.guest_read::<i32>(pptl + 4).unwrap_or(0),
                );
                let wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get(&hps) {
                    let (x1, y1) = ps.current_pos;
                    let color = ps.color;
                    let fill = control >= 2; // DRO_FILL or DRO_OUTLINEFILL
                    let hwnd = ps.hwnd;
                    let frame_hwnd = wm.client_to_frame(hwnd);
                    if let Some(ref sender) = wm.gui_tx {
                        let _ = sender.send(GUIMessage::DrawBox {
                            handle: frame_hwnd, x1, y1, x2, y2, color, fill
                        });
                    }
                }
                ApiResult::Normal(1) // GPI_OK
            }
            398 => {
                // GpiLine(HPS hps, PPOINTL pptl)
                let hps = read_stack(4);
                let pptl = read_stack(8);
                let (x2, y2) = (
                    self.guest_read::<i32>(pptl).unwrap_or(0),
                    self.guest_read::<i32>(pptl + 4).unwrap_or(0),
                );
                let mut wm = self.shared.window_mgr.lock_or_recover();
                if let Some(ps) = wm.ps_map.get(&hps) {
                    let (x1, y1) = ps.current_pos;
                    let color = ps.color;
                    let hwnd = ps.hwnd;
                    let frame_hwnd = wm.client_to_frame(hwnd);
                    if let Some(ref sender) = wm.gui_tx {
                        let _ = sender.send(GUIMessage::DrawLine {
                            handle: frame_hwnd, x1, y1, x2, y2, color
                        });
                    }
                }
                // Update current position
                if let Some(ps) = wm.ps_map.get_mut(&hps) {
                    ps.current_pos = (x2, y2);
                }
                ApiResult::Normal(1) // GPI_OK
            }
            359 => {
                // GpiCharStringAt(HPS hps, PPOINTL pptl, LONG lCount, PCH pchString)
                let hps = read_stack(4);
                let pptl = read_stack(8);
                let count = read_stack(12) as usize;
                let pch = read_stack(16);
                let (x, y) = (
                    self.guest_read::<i32>(pptl).unwrap_or(0),
                    self.guest_read::<i32>(pptl + 4).unwrap_or(0),
                );
                let text: Vec<u8> = (0..count).map(|i| {
                    self.guest_read::<u8>(pch + i as u32).unwrap_or(0)
                }).collect();
                let text_str = String::from_utf8_lossy(&text).to_string();
                let mut wm = self.shared.window_mgr.lock_or_recover();
                let color = wm.ps_map.get(&hps).map(|ps| ps.color).unwrap_or(0);
                let ps_hwnd = wm.ps_map.get(&hps).map(|ps| ps.hwnd).unwrap_or(0);
                let hwnd = wm.client_to_frame(ps_hwnd);
                if let Some(ref sender) = wm.gui_tx {
                    let _ = sender.send(GUIMessage::DrawText {
                        handle: hwnd, x, y, text: text_str, color,
                    });
                }
                // Update current position (advance x by character width * count)
                if let Some(ps) = wm.ps_map.get_mut(&hps) {
                    ps.current_pos = (x + (count as i32 * 8), y);
                }
                ApiResult::Normal(1) // GPI_OK
            }
            389 => {
                // GpiErase(HPS hps)
                let hps = read_stack(4);
                let wm = self.shared.window_mgr.lock_or_recover();
                let ps_hwnd = wm.ps_map.get(&hps).map(|ps| ps.hwnd).unwrap_or(0);
                let frame_hwnd = wm.client_to_frame(ps_hwnd);
                if let Some(ref sender) = wm.gui_tx {
                    let _ = sender.send(GUIMessage::ClearBuffer { handle: frame_hwnd });
                }
                ApiResult::Normal(1)
            }
            _ => {
                warn!("Warning: Unknown PMGPI Ordinal {} on VCPU {}", ordinal, vcpu_id);
                ApiResult::Normal(0)
            }
        }
    }
}
