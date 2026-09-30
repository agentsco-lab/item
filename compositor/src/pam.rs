//! The PIN checked as phosh checks it: PAM's `phosh` service (common-auth,
//! pam_unix, which asks the setgid unix_chkpwd to check the user's own
//! password - the PIN on the port), for the session's user.
//!
//! libpam is opened at run time (the cross build links against no libpam).
//! A check runs on a thread of its own: pam_unix waits about 2 s after a
//! wrong one. The PIN is never logged, and its copies are zeroed.

use std::ffi::{c_char, c_int, c_void, CStr, CString};

const PAM_SUCCESS: c_int = 0;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_CONV_ERR: c_int = 19;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

#[repr(C)]
struct PamConv {
    conv: extern "C" fn(c_int, *mut *const PamMessage, *mut *mut PamResponse, *mut c_void) -> c_int,
    appdata: *mut c_void,
}

type Start = unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *mut *mut c_void) -> c_int;
type Auth = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type End = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;

/// The conversation: every hidden or shown prompt answered with the PIN.
extern "C" fn conversation(n: c_int, msgs: *mut *const PamMessage, resp: *mut *mut PamResponse, appdata: *mut c_void) -> c_int {
    if n <= 0 || msgs.is_null() || resp.is_null() {
        return PAM_CONV_ERR;
    }
    unsafe {
        let pin = &*(appdata as *const CString);
        let out = libc::calloc(n as usize, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
        if out.is_null() {
            return PAM_CONV_ERR;
        }
        for i in 0..n as usize {
            // Linux-PAM passes an array of pointers to messages.
            let m = &**msgs.add(i);
            if m.msg_style == PAM_PROMPT_ECHO_OFF || m.msg_style == PAM_PROMPT_ECHO_ON {
                (*out.add(i)).resp = libc::strdup(pin.as_ptr());
            }
        }
        *resp = out;
    }
    PAM_SUCCESS
}

/// Whether `pin` is the session user's: PAM's answer.
pub fn check(pin: &str) -> bool {
    let user = std::env::var("USER").ok().or_else(|| unsafe {
        let pw = libc::getpwuid(libc::getuid());
        (!pw.is_null()).then(|| CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned())
    });
    let Some(user) = user else { return false };
    unsafe {
        let lib = libc::dlopen(c"libpam.so.0".as_ptr(), libc::RTLD_NOW);
        if lib.is_null() {
            tracing::warn!("pam: no libpam");
            return false;
        }
        let sym = |n: &CStr| libc::dlsym(lib, n.as_ptr());
        let (start, auth, end) = (sym(c"pam_start"), sym(c"pam_authenticate"), sym(c"pam_end"));
        if start.is_null() || auth.is_null() || end.is_null() {
            return false;
        }
        let (start, auth, end): (Start, Auth, End) = (std::mem::transmute(start), std::mem::transmute(auth), std::mem::transmute(end));
        let mut secret = CString::new(pin).unwrap_or_default();
        let conv = PamConv { conv: conversation, appdata: &secret as *const CString as *mut c_void };
        let service = c"phosh";
        let user_c = CString::new(user.clone()).unwrap_or_default();
        let mut handle: *mut c_void = std::ptr::null_mut();
        let mut rc = start(service.as_ptr(), user_c.as_ptr(), &conv, &mut handle);
        if rc == PAM_SUCCESS {
            rc = auth(handle, 0);
            end(handle, rc);
        }
        // Zero the PIN's copy before it goes.
        let bytes = secret.as_bytes().len();
        let p = secret.as_ptr() as *mut u8;
        std::ptr::write_bytes(p, 0, bytes);
        secret = CString::default();
        drop(secret);
        tracing::info!("pam: {user}: {}", if rc == PAM_SUCCESS { "accepted" } else { "refused" });
        rc == PAM_SUCCESS
    }
}
