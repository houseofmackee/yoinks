//! The bridge the Shortcuts actions (`swift/Intents.swift`) call: one C
//! function that answers a request in the command line's JSON format.

use std::ffi::{CStr, CString, c_char};

use crate::ipc::{Request, Response, server};

/// Answers the JSON request `request` on the main thread and returns the
/// JSON response, which the caller frees with [`yoinks_free_string`].
///
/// # Safety
///
/// `request` must be a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yoinks_request(request: *const c_char) -> *mut c_char {
    let response = if objc2::MainThreadMarker::new().is_none() {
        Response::error("Shortcuts actions must run on the main thread")
    } else if request.is_null() {
        Response::error("no request")
    } else {
        // SAFETY: Per the function's contract.
        let json = unsafe { CStr::from_ptr(request) }.to_string_lossy();
        match serde_json::from_str::<Request>(&json) {
            Ok(request) => server::handle(request),
            Err(error) => Response::error(format!("bad request: {error}")),
        }
    };
    let json = serde_json::to_string(&response).unwrap_or_default();
    CString::new(json.replace('\0', " ")).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Frees a string returned by [`yoinks_request`].
///
/// # Safety
///
/// `string` must come from `yoinks_request` and be freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yoinks_free_string(string: *mut c_char) {
    if !string.is_null() {
        // SAFETY: Per the function's contract.
        drop(unsafe { CString::from_raw(string) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(json: &str) -> Response {
        let request = CString::new(json).unwrap();
        // SAFETY: A valid C string; the reply is freed below.
        let reply = unsafe { yoinks_request(request.as_ptr()) };
        // SAFETY: `yoinks_request` returns a valid C string or null.
        let text = unsafe { CStr::from_ptr(reply) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: Freed once, as required.
        unsafe { yoinks_free_string(reply) };
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn answers_off_the_main_thread_with_an_error() {
        // Unit tests run off the main thread, like a misbehaving caller.
        let response = ask(r#"{"cmd":"list"}"#);
        assert!(!response.ok);
        assert!(response.error.unwrap().contains("main thread"));
    }

    #[test]
    fn freeing_null_is_harmless() {
        // SAFETY: Null is explicitly allowed.
        unsafe { yoinks_free_string(std::ptr::null_mut()) };
    }
}
