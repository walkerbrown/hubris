// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Server for managing the Sidecar sequencing process.

#![no_std]
#![no_main]

mod controller_fpga;

use ringbuf::*;
use userlib::*;

use drv_spi_api as spi_api;
use drv_i2c_api::{I2cDevice, ResponseCode};
use drv_sidecar_seq_api::{PowerState, SeqError};
use idol_runtime::{NotificationHandler, RequestError};

task_slot!(GPIO, gpio_driver);
task_slot!(I2C, i2c_driver);
task_slot!(SPI, spi_driver);

mod payload;

include!(concat!(env!("OUT_DIR"), "/i2c_config.rs"));
use i2c_config::devices;

#[derive(Copy, Clone, PartialEq)]
enum Tofino2Vid {
    Invalid,
    V0P915,
    V0P885,
    V0P860,
    V0P840,
    V0P825,
    V0P810,
    V0P785,
    V0P755,
}

#[derive(Copy, Clone, PartialEq)]
enum Trace {
    A2,
    GetState,
    SetState(PowerState, PowerState),
    LoadClockConfig,
    ClockConfigWrite(usize),
    ClockConfigSuccess(usize),
    ClockConfigFailed(usize, ResponseCode),
    ValidControllerIdent,
    SetTofinoEn(u8),
    SampledVid(u8),
    SetVddCoreVout(userlib::units::Volts),
    Done,
    None,
}

ringbuf!(Trace, 64, Trace::None);

const TIMER_MASK: u32 = 1 << 0;
const TIMER_INTERVAL: u64 = 1000;

struct ServerImpl {
    state: PowerState,
    clockgen: I2cDevice,
    led: drv_stm32h7_gpio_api::PinSet,
    led_on: bool,
    deadline: u64,
    controller: controller_fpga::ControllerFpga,
    vid: Tofino2Vid,
}

impl ServerImpl {
    fn led_init(&mut self) {
        use drv_stm32h7_gpio_api::*;

        let gpio_driver = GPIO.get_task_id();
        let gpio_driver = Gpio::from(gpio_driver);

        // Make the LED an output.
        gpio_driver
            .configure_output(
                self.led,
                OutputType::PushPull,
                Speed::High,
                Pull::None,
            )
            .unwrap();
    }

    fn led_on(&mut self) {
        use drv_stm32h7_gpio_api::*;

        let gpio_driver = GPIO.get_task_id();
        let gpio_driver = Gpio::from(gpio_driver);
        gpio_driver.set_to(self.led, true).unwrap();
        self.led_on = true;
    }

    fn led_off(&mut self) {
        use drv_stm32h7_gpio_api::*;

        let gpio_driver = GPIO.get_task_id();
        let gpio_driver = Gpio::from(gpio_driver);
        gpio_driver.set_to(self.led, false).unwrap();
        self.led_on = false;
    }

    fn led_toggle(&mut self) {
        if self.led_on {
            self.led_off();
        } else {
            self.led_on();
        }
    }

    fn tofino_enabled(&mut self) -> bool {
        use controller_fpga::*;

        let mut en = [1u8];
        self.controller.read_bytes(Addr::TOFINO_EN, &mut en).unwrap();
        return en[0] == 1;
    }

    fn set_tofino_enabled(&mut self, enabled: bool) {
        use controller_fpga::*;

        let mut en = [if enabled { 1u8 } else { 0u8 }];
        self.controller.set_bytes(Addr::TOFINO_EN, &mut en).unwrap();
        ringbuf_entry!(Trace::SetTofinoEn(en[0]));
    }

    fn get_tofino_seq_state(&mut self) -> u8 {
        use controller_fpga::*;

        let mut seq_state = [0u8];
        self.controller.read_bytes(Addr::TOFINO_SEQ_STATE, &mut seq_state).unwrap();
        return seq_state[0];
    }

    fn get_tofino_seq_error(&mut self) -> u8 {
        use controller_fpga::*;

        let mut seq_error = [0u8];
        self.controller.read_bytes(Addr::TOFINO_SEQ_ERROR, &mut seq_error).unwrap();
        return seq_error[0];
    }

    fn get_tofino_vid(&mut self) {
        use controller_fpga::*;

        let mut vid = [0u8];
        self.controller.read_bytes(Addr::TOFINO_VID, &mut vid).unwrap();

        self.vid = match vid[0] {
            0b1111 => Tofino2Vid::V0P915,
            0b1110 => Tofino2Vid::V0P885,
            0b1101 => Tofino2Vid::V0P860,
            0b1100 => Tofino2Vid::V0P840,
            0b1011 => Tofino2Vid::V0P825,
            0b1010 => Tofino2Vid::V0P810,
            0b1001 => Tofino2Vid::V0P785,
            0b1000 => Tofino2Vid::V0P755,
            _ => Tofino2Vid::Invalid,
        };

        ringbuf_entry!(Trace::SampledVid(vid[0]));
    }

    fn apply_vid(&mut self) {
        use userlib::units::Volts;

        fn set_vout(value: Volts) {
            use drv_i2c_devices::raa229618::Raa229618;
            let i2c = I2C.get_task_id();

            let (device, rail) = i2c_config::pmbus::tf2_vddcore(i2c);
            let mut vddcore = Raa229618::new(&device, rail);

            vddcore.set_vout(value).unwrap();
            ringbuf_entry!(Trace::SetVddCoreVout(value));
        }

        match self.vid {
            Tofino2Vid::V0P915 => set_vout(Volts(0.915)),
            Tofino2Vid::V0P885 => set_vout(Volts(0.885)),
            Tofino2Vid::V0P860 => set_vout(Volts(0.860)),
            Tofino2Vid::V0P840 => set_vout(Volts(0.840)),
            Tofino2Vid::V0P825 => set_vout(Volts(0.825)),
            Tofino2Vid::V0P810 => set_vout(Volts(0.810)),
            Tofino2Vid::V0P785 => set_vout(Volts(0.785)),
            Tofino2Vid::V0P755 => set_vout(Volts(0.755)),
            Tofino2Vid::Invalid => panic!(),
        }
    }
}

impl idl::InOrderSequencerImpl for ServerImpl {
    fn get_state(
        &mut self,
        _: &RecvMessage,
    ) -> Result<PowerState, RequestError<SeqError>> {
        ringbuf_entry!(Trace::GetState);
        Ok(self.state)
    }

    fn set_state(
        &mut self,
        _: &RecvMessage,
        state: PowerState,
    ) -> Result<(), RequestError<SeqError>> {
        ringbuf_entry!(Trace::SetState(self.state, state));

        match (self.state, state) {
            (PowerState::A2, PowerState::A0) => {
                //
                // Initiate the start up sequence.
                //
                self.set_tofino_enabled(true);

                //
                // Wait for VID bits to be valid.
                //
                while self.get_tofino_seq_state() < 9 {
                    hl::sleep_for(10);
                }

                self.get_tofino_vid();
                self.apply_vid();

                self.state = PowerState::A0;
                Ok(())
            }

            (PowerState::A0, PowerState::A2) => {
                self.set_tofino_enabled(false);
                Ok(())
            }

            _ => Err(RequestError::Runtime(SeqError::IllegalTransition)),
        }
    }

    fn load_clock_config(
        &mut self,
        _: &RecvMessage,
    ) -> Result<(), RequestError<SeqError>> {
        ringbuf_entry!(Trace::LoadClockConfig);

        let mut packet = 0;

        payload::idt8a3xxxx_payload(|buf| {
            ringbuf_entry!(Trace::ClockConfigWrite(packet));
            match self.clockgen.write(buf) {
                Err(err) => {
                    ringbuf_entry!(Trace::ClockConfigFailed(packet, err));
                    Err(SeqError::ClockConfigFailed)
                }

                Ok(_) => {
                    ringbuf_entry!(Trace::ClockConfigSuccess(packet));
                    packet += 1;
                    Ok(())
                }
            }
        })?;

        Ok(())
    }
}

impl NotificationHandler for ServerImpl {
    fn current_notification_mask(&self) -> u32 {
        TIMER_MASK
    }

    fn handle_notification(&mut self, _bits: u32) {
        self.deadline += TIMER_INTERVAL;
        self.led_toggle();
        sys_set_timer(Some(self.deadline), TIMER_MASK);
    }
}

#[export_name = "main"]
fn main() -> ! {
    let task = I2C.get_task_id();
    let spi = spi_api::Spi::from(SPI.get_task_id());
    let controller =
        controller_fpga::ControllerFpga::new(
            spi.device(CONTROLLER_FPGA_SPI_DEVICE));

    ringbuf_entry!(Trace::A2);

    if controller.valid_ident() {
        ringbuf_entry!(Trace::ValidControllerIdent);
    }

    let mut buffer = [0; idl::INCOMING_SIZE];
    let deadline = sys_get_timer().now;

    //
    // This will put our timer in the past, and should immediately kick us.
    //
    sys_set_timer(Some(deadline), TIMER_MASK);

    let mut server = ServerImpl {
        state: PowerState::A2,
        clockgen: devices::idt8a34001(task)[0],
        led: drv_stm32h7_gpio_api::Port::C.pin(3),
        led_on: false,
        deadline,
        controller: controller,
        vid: Tofino2Vid::Invalid,
    };

    server.led_init();

    loop {
        ringbuf_entry!(Trace::Done);
        idol_runtime::dispatch_n(&mut buffer, &mut server);
    }
}

cfg_if::cfg_if! {
    if #[cfg(target_board = "sidecar-1")] {
        const CONTROLLER_FPGA_SPI_DEVICE: u8 = 0;
    } else {
        compiler_error!("unsupported target board");
    }
}

mod idl {
    use super::{PowerState, SeqError};

    include!(concat!(env!("OUT_DIR"), "/server_stub.rs"));
}
