// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![no_std]
#![no_main]

use idol_runtime::{RequestError, LenLimit, Leased, W, R};
use userlib::*;

use ringbuf::{ringbuf, ringbuf_entry};
use spdm::{
    config::NUM_SLOTS,
    crypto::{FakeSigner, FilledSlot},
};

#[derive(Copy, Clone, Debug, FromPrimitive)]
#[repr(u32)]
pub enum SpdmError {
    SomeSortOfError = 1,
}

impl From<u32> for SpdmError {
    fn from(x: u32) -> Self {
        match x {
            1 => SpdmError::SomeSortOfError,
            _ => panic!(),
        }
    }
}

impl From<SpdmError> for u16 {
    fn from(x: SpdmError) -> Self {
        x as u16
    }
}

/// Record the types and sizes of the messages sent and received by this server
#[derive(Copy, Clone, PartialEq, Debug)]
enum LogMsg {
    // Static initializer
    Init,
    // _Received { code: u8, size: u16 },
    // _Sent { code: u8, size: u16 },
    State(&'static str),
}

#[export_name = "main"]
fn main() -> ! {
    ringbuf!(LogMsg, 16, LogMsg::Init);
    const EMPTY_SLOT: Option<FilledSlot<'_, FakeSigner>> = None;
    let slots = [EMPTY_SLOT; NUM_SLOTS];
    let mut responder = spdm::Responder::new(slots);
    ringbuf_entry!(LogMsg::State(responder.state().name()));

    let mut buffer = [0; idl::INCOMING_SIZE];
    let mut server = ServerImpl {
        responder: responder,
    };

    loop {
     //   idol_runtime::dispatch(&mut buffer, &mut server);
    }
}

struct ServerImpl<'a> {
    responder: spdm::Responder<'a, FakeSigner>,
}

impl idl::InOrderSpdmImpl for ServerImpl<'_> {
    fn exchange(&mut self,
        _: &RecvMessage,
        source: LenLimit<Leased<R, [u8]>, 256>,
        sink: LenLimit<Leased<W, [u8]>, 256>,
    ) -> Result<(), RequestError<SpdmError>> {
        Ok(())
    }
}

mod idl {
    use super::{SpdmError};

    include!(concat!(env!("OUT_DIR"), "/server_stub.rs"));
}
