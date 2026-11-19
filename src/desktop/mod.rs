// SPDX-License-Identifier: GPL-3.0-only

//! One desktop per session (ADR-0052): the desktop server process and the
//! link every Warpine process of the session keeps to it.
//!
//! - [`pixmap`]: frame pictures in sealed memfds, shared with the server;
//! - [`proto`]: the messages, strictly decoded;
//! - [`transport`]: the `SOCK_SEQPACKET` socket in the session directory;
//! - [`model`]: the server's stacking, activation and input routing;
//! - [`client`]: a Warpine process's connection (start the server if
//!   needed, handshake, reader thread);
//! - the server itself (SDL window, compositing) is `gui::desktop_server`.

pub mod pixmap;
pub mod proto;
pub mod transport;
pub mod model;
pub mod calls;
pub mod client;
