use core::cell::RefCell;

use critical_section::Mutex;
use esp32c2 as pac;
use esp32c2_hal as hal;
use esp32c2_hal::interrupt::TrapFrame;
use esp32c2_hal::prelude::*;
use hal::peripherals::Interrupt;
use hal::systimer::{Alarm, Periodic, Target};

use crate::{binary, preempt::preempt::task_switch};
use crate::{trace, unwrap};

pub const TICKS_PER_SECOND: u64 = 16_000_000;

pub const COUNTER_BIT_MASK: u64 = 0x000F_FFFF_FFFF_FFFF;

const TIMER_DELAY: fugit::HertzU32 = fugit::HertzU32::from_raw(crate::CONFIG.tick_rate_hz);

static ALARM0: Mutex<RefCell<Option<Alarm<Periodic, 0>>>> = Mutex::new(RefCell::new(None));

pub fn setup_timer_isr(systimer: Alarm<Target, 0>) {
    let alarm0 = systimer.into_periodic();
    alarm0.set_period(TIMER_DELAY.into());
    alarm0.clear_interrupt();
    alarm0.interrupt_enable(true);

    critical_section::with(|cs| ALARM0.borrow_ref_mut(cs).replace(alarm0));

    unwrap!(esp32c2_hal::interrupt::enable(
        Interrupt::SYSTIMER_TARGET0,
        hal::interrupt::Priority::Priority1,
    ));

    #[cfg(feature = "wifi")]
    unwrap!(esp32c2_hal::interrupt::enable(
        Interrupt::WIFI_MAC,
        hal::interrupt::Priority::Priority1
    ));

    #[cfg(feature = "wifi")]
    unwrap!(esp32c2_hal::interrupt::enable(
        Interrupt::WIFI_PWR,
        hal::interrupt::Priority::Priority1
    ));

    #[cfg(feature = "ble")]
    {
        unwrap!(esp32c2_hal::interrupt::enable(
            Interrupt::LP_TIMER,
            hal::interrupt::Priority::Priority1
        ));
        unwrap!(esp32c2_hal::interrupt::enable(
            Interrupt::BT_MAC,
            hal::interrupt::Priority::Priority1
        ));
    }

    unwrap!(esp32c2_hal::interrupt::enable(
        Interrupt::FROM_CPU_INTR3,
        hal::interrupt::Priority::Priority1,
    ));

    unsafe {
        esp32c2_hal::riscv::interrupt::enable();
    }

    while unsafe { crate::preempt::FIRST_SWITCH.load(core::sync::atomic::Ordering::Relaxed) } {}
}

#[cfg(feature = "wifi")]
#[interrupt]
fn WIFI_MAC() {
    unsafe {
        let (fnc, arg) = crate::wifi::os_adapter::ISR_INTERRUPT_1;

        trace!("interrupt WIFI_MAC {:?} {:?}", fnc, arg);

        if !fnc.is_null() {
            let fnc: fn(*mut binary::c_types::c_void) = core::mem::transmute(fnc);
            fnc(arg);
        }

        trace!("interrupt 1 done");
    };
}

#[cfg(feature = "wifi")]
#[interrupt]
fn WIFI_PWR() {
    unsafe {
        let (fnc, arg) = crate::wifi::os_adapter::ISR_INTERRUPT_1;

        trace!("interrupt WIFI_PWR {:?} {:?}", fnc, arg);

        if !fnc.is_null() {
            let fnc: fn(*mut binary::c_types::c_void) = core::mem::transmute(fnc);
            fnc(arg);
        }

        trace!("interrupt 1 done");
    };
}

#[cfg(feature = "ble")]
#[interrupt]
fn LP_TIMER() {
    unsafe {
        trace!("LP_TIMER interrupt");

        let (fnc, arg) = crate::ble::npl::ble_os_adapter_chip_specific::ISR_INTERRUPT_7;

        trace!("interrupt LP_TIMER {:?} {:?}", fnc, arg);

        if !fnc.is_null() {
            trace!("interrupt LP_TIMER call");

            let fnc: fn(*mut binary::c_types::c_void) = core::mem::transmute(fnc);
            fnc(arg);
            trace!("LP_TIMER done");
        }

        trace!("interrupt LP_TIMER done");
    };
}

#[cfg(feature = "ble")]
#[interrupt]
fn BT_MAC() {
    unsafe {
        trace!("BT_MAC interrupt");

        let (fnc, arg) = crate::ble::npl::ble_os_adapter_chip_specific::ISR_INTERRUPT_4;

        trace!("interrupt BT_MAC {:?} {:?}", fnc, arg);

        if !fnc.is_null() {
            trace!("interrupt BT_MAC call");

            let fnc: fn(*mut binary::c_types::c_void) = core::mem::transmute(fnc);
            fnc(arg);
            trace!("BT_MAC done");
        }

        trace!("interrupt BT_MAC done");
    };
}

#[interrupt]
fn SYSTIMER_TARGET0(trap_frame: &mut TrapFrame) {
    // clear the systimer intr
    critical_section::with(|cs| {
        unwrap!(ALARM0.borrow_ref_mut(cs).as_mut()).clear_interrupt();
    });

    task_switch(trap_frame);
}

#[interrupt]
fn FROM_CPU_INTR3(trap_frame: &mut TrapFrame) {
    unsafe {
        // clear ETS_FROM_CPU_INTR3
        (&*pac::SYSTEM::PTR)
            .cpu_intr_from_cpu_3
            .modify(|_, w| w.cpu_intr_from_cpu_3().clear_bit());
    }

    critical_section::with(|cs| {
        let mut alarm0 = ALARM0.borrow_ref_mut(cs);
        let alarm0 = unwrap!(alarm0.as_mut());

        alarm0.set_period(TIMER_DELAY.into());
        alarm0.clear_interrupt();
    });

    task_switch(trap_frame);
}

pub fn yield_task() {
    unsafe {
        (&*pac::SYSTEM::PTR)
            .cpu_intr_from_cpu_3
            .modify(|_, w| w.cpu_intr_from_cpu_3().set_bit());
    }
}

/// Current systimer count value
/// A tick is 1 / 16_000_000 seconds
pub fn get_systimer_count() -> u64 {
    critical_section::with(|_| unsafe {
        let systimer = &(*pac::SYSTIMER::ptr());

        systimer.unit0_op.write(|w| w.bits(1 << 30));

        // wait for value available
        loop {
            let valid = (systimer.unit0_op.read().bits() >> 29) & 1;
            if valid != 0 {
                break;
            }
        }

        let value_lo = systimer.unit0_value_lo.read().bits() as u64;
        let value_hi = (systimer.unit0_value_hi.read().bits() as u64) << 32;

        (value_lo | value_hi) as u64
    })
}
