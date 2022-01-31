// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![no_std]
#![no_main]

use idol_runtime::{Leased, LenLimit, RequestError, R, W};
use userlib::*;

use ringbuf::{ringbuf, ringbuf_entry};
use spdm::{
    config::NUM_SLOTS,
    crypto::{FakeSigner, FilledSlot},
    responder::AllStates,
};

#[derive(Copy, Clone, Debug, FromPrimitive)]
#[repr(u32)]
pub enum SpdmError {
    ResponderReset = 1,
}

impl From<u32> for SpdmError {
    fn from(x: u32) -> Self {
        match x {
            1 => SpdmError::ResponderReset,
            _ => panic!(),
        }
    }
}

impl From<SpdmError> for u16 {
    fn from(x: SpdmError) -> Self {
        x as u16
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum State {
    Error,
    Version,
    Capabilities,
    Algorithms,
    IdAuth,
    Challenge,
}

impl From<&AllStates> for State {
    fn from(state: &AllStates) -> Self {
        match state {
            AllStates::Error => State::Error,
            AllStates::Version(_) => State::Version,
            AllStates::Capabilities(_) => State::Capabilities,
            AllStates::Algorithms(_) => State::Algorithms,
            AllStates::IdAuth(_) => State::IdAuth,
            AllStates::Challenge(_) => State::Challenge,
        }
    }
}

/// Record the types and sizes of the messages sent and received by this server
#[derive(Copy, Clone, PartialEq, Debug)]
enum LogMsg {
    // Static initializer
    Init,
    State(State),
    Recv {
        sender: TaskId,
        op: u32,
        msg_len: usize,
    },
}

ringbuf!(LogMsg, 16, LogMsg::Init);

#[export_name = "main"]
fn main() -> ! {
    let mut buffer = [0; idl::INCOMING_SIZE];
    let mut server = ServerImpl::new();

    loop {
        idol_runtime::dispatch(&mut buffer, &mut server);
    }
}

struct ServerImpl<'a> {
    responder: spdm::Responder<'a, FakeSigner>,
}

impl<'a> ServerImpl<'a> {
    fn new() -> ServerImpl<'a> {
        const EMPTY_SLOT: Option<FilledSlot<'_, FakeSigner>> = None;
        let slots = [EMPTY_SLOT; NUM_SLOTS];
        let responder = spdm::Responder::new(slots);
        ringbuf_entry!(LogMsg::State(responder.state().into()));
        ServerImpl { responder }
    }
}

impl idl::InOrderSpdmImpl for ServerImpl<'_> {
    fn exchange(
        &mut self,
        msg: &RecvMessage,
        source: LenLimit<Leased<R, [u8]>, 256>,
        sink: LenLimit<Leased<W, [u8]>, 256>,
    ) -> Result<(), RequestError<SpdmError>> {
        ringbuf_entry!(LogMsg::Recv {
            sender: msg.sender,
            op: msg.operation,
            msg_len: msg.message_len,
        });

        let mut req = [0u8; 256];
        let mut rsp = [0u8; 256];
        (*source).read_range(0..source.len(), &mut req).unwrap();

        let (reply, res) =
            self.responder.handle_msg(&req[..source.len()], &mut rsp);
        ringbuf_entry!(LogMsg::State(self.responder.state().into()));

        // There was a protocol error. Just go back to the initial state.
        if res.is_err() {
            self.responder.reset();
            ringbuf_entry!(LogMsg::State(self.responder.state().into()));

            // A reply is always valid, even in the case of an error, since we want
            // to inform the requester of any SPDM errors that should go over the wire. In some
            // cases, reply may be of zero length however, indicating that nothing should be
            // sent to the responder and any connections should be closed. In this
            // case we return an Error.
            //
            // TODO: Map actual underlying errors?
            if reply.is_empty() {
                return Err(SpdmError::ResponderReset.into());
            }
        }

        sink.write_range(0..reply.len(), reply).unwrap();

        Ok(())
    }
}

mod idl {
    use super::SpdmError;

    include!(concat!(env!("OUT_DIR"), "/server_stub.rs"));
}
