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
use smithay::reexports::wayland_protocols::wp::text_input::zv3::server::{
    zwp_text_input_manager_v3::ZwpTextInputManagerV3,
    zwp_text_input_v3::{self, ZwpTextInputV3},
};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use smithay::wayland::text_input::{TextInputManagerState, TextInputUserData};

use crate::state::State;

// ---- text-input v3, watched -------------------------------------------------
//
// smithay's text-input keeps a client's enable/disable only while an input
// method (stevia) is bound, and item withholds that global in item mode
// (state.rs). So the requests pass through here on their way to smithay:
// an enable committed by the window with the focus means a text field took
// the cursor, a disable that it let go - what item's own keyboard rises and
// hides on (state.rs's text_field).

smithay::reexports::wayland_server::delegate_global_dispatch!(State: [ZwpTextInputManagerV3: ()] => TextInputManagerState);
smithay::reexports::wayland_server::delegate_dispatch!(State: [ZwpTextInputManagerV3: ()] => TextInputManagerState);

impl Dispatch<ZwpTextInputV3, TextInputUserData> for State {
    fn request(state: &mut State, client: &Client, resource: &ZwpTextInputV3, request: zwp_text_input_v3::Request, data: &TextInputUserData, dh: &DisplayHandle, init: &mut DataInit<'_, State>) {
        match request {
            zwp_text_input_v3::Request::Enable => state.field_pending = Some(true),
            zwp_text_input_v3::Request::Disable => state.field_pending = Some(false),
            zwp_text_input_v3::Request::Commit => {
                if let Some(on) = state.field_pending.take() {
                    state.text_field(on, &resource.id());
                }
            }
            _ => {}
        }
        <TextInputManagerState as Dispatch<ZwpTextInputV3, TextInputUserData, State>>::request(state, client, resource, request, data, dh, init);
    }

    fn destroyed(state: &mut State, client: smithay::reexports::wayland_server::backend::ClientId, resource: &ZwpTextInputV3, data: &TextInputUserData) {
        <TextInputManagerState as Dispatch<ZwpTextInputV3, TextInputUserData, State>>::destroyed(state, client, resource, data);
    }
}

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
