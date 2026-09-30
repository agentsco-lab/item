//! The globals phosh-osk-stevia waits for before it takes the input method
//! (its PosWayland is "ready" only with all of them bound):
//! - phoc's device state (`zphoc_device_state_v1`), from the port's XML: the
//!   Duo has no tablet-mode or lid switch that anyone reports, and no
//!   hardware keyboard, so its capabilities are none and its switches say
//!   nothing;
//! - wlr's foreign toplevel manager: bound, and no toplevels announced yet
//!   (stevia uses it to know the app it types into);
//! - wlr's data control, which smithay has (state.rs).

use smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::{
    zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use crate::state::State;

#[allow(non_upper_case_globals, non_camel_case_types, dead_code, clippy::all)]
pub mod device_state {
    use wayland_server;
    pub mod __interfaces {
        wayland_scanner::generate_interfaces!("protocols/phoc-device-state-unstable-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_server_code!("protocols/phoc-device-state-unstable-v1.xml");
}

use device_state::zphoc_device_state_v1::{self, ZphocDeviceStateV1};
use device_state::zphoc_lid_switch_v1::ZphocLidSwitchV1;
use device_state::zphoc_tablet_mode_switch_v1::ZphocTabletModeSwitchV1;

pub fn create_globals(dh: &DisplayHandle) {
    dh.create_global::<State, ZphocDeviceStateV1, ()>(2, ());
    dh.create_global::<State, ZwlrForeignToplevelManagerV1, ()>(3, ());
}

impl GlobalDispatch<ZphocDeviceStateV1, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, resource: New<ZphocDeviceStateV1>, _: &(), init: &mut DataInit<'_, State>) {
        let state = init.init(resource, ());
        state.capabilities(zphoc_device_state_v1::Capability::empty());
    }
}

impl Dispatch<ZphocDeviceStateV1, ()> for State {
    fn request(_: &mut State, _: &Client, _: &ZphocDeviceStateV1, request: zphoc_device_state_v1::Request, _: &(), _: &DisplayHandle, init: &mut DataInit<'_, State>) {
        match request {
            zphoc_device_state_v1::Request::GetTabletModeSwitch { id } => {
                init.init(id, ());
            }
            zphoc_device_state_v1::Request::GetLidSwitch { id } => {
                init.init(id, ());
            }
        }
    }
}

impl Dispatch<ZphocTabletModeSwitchV1, ()> for State {
    fn request(_: &mut State, _: &Client, _: &ZphocTabletModeSwitchV1, _: device_state::zphoc_tablet_mode_switch_v1::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {}
}

impl Dispatch<ZphocLidSwitchV1, ()> for State {
    fn request(_: &mut State, _: &Client, _: &ZphocLidSwitchV1, _: device_state::zphoc_lid_switch_v1::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {}
}

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn bind(_: &mut State, _: &DisplayHandle, _: &Client, resource: New<ZwlrForeignToplevelManagerV1>, _: &(), init: &mut DataInit<'_, State>) {
        init.init(resource, ());
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn request(_: &mut State, _: &Client, manager: &ZwlrForeignToplevelManagerV1, request: zwlr_foreign_toplevel_manager_v1::Request, _: &(), _: &DisplayHandle, _: &mut DataInit<'_, State>) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            if manager.is_alive() {
                manager.finished();
            }
        }
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn request(
        _: &mut State,
        _: &Client,
        _: &ZwlrForeignToplevelHandleV1,
        _: smithay::reexports::wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
    }
}
