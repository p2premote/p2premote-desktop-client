//! Thin macOS 13+ ServiceManagement bridge.
//!
//! Keeping this bridge in Rust avoids shipping an unsigned script or requiring
//! Xcode/Swift on an end-user machine. The daemon property list is embedded in
//! the signed application bundle at `Contents/Library/LaunchDaemons`.

#![cfg(target_os = "macos")]

use anyhow::{anyhow, Result};
use std::ffi::{c_char, c_void, CStr, CString};

const DAEMON_PLIST_NAME: &str = "top.p2premote.service.plist";

type ObjcId = *mut c_void;
type ObjcSel = *mut c_void;

#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> ObjcId;
    fn sel_registerName(name: *const c_char) -> ObjcSel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> ObjcId;
    fn objc_autoreleasePoolPop(pool: ObjcId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationStatus {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
    Unknown(isize),
}

impl RegistrationStatus {
    fn from_raw(value: isize) -> Self {
        match value {
            0 => Self::NotRegistered,
            1 => Self::Enabled,
            2 => Self::RequiresApproval,
            3 => Self::NotFound,
            other => Self::Unknown(other),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRegistered => "not-registered",
            Self::Enabled => "enabled",
            Self::RequiresApproval => "requires-approval",
            Self::NotFound => "not-found",
            Self::Unknown(_) => "unknown",
        }
    }
}

unsafe fn selector(name: &str) -> ObjcSel {
    let name = CString::new(name).expect("Objective-C selector contains no NUL");
    sel_registerName(name.as_ptr())
}

unsafe fn class(name: &str) -> Result<ObjcId> {
    let name = CString::new(name).expect("Objective-C class name contains no NUL");
    let value = objc_getClass(name.as_ptr());
    if value.is_null() {
        Err(anyhow!("Objective-C class is unavailable"))
    } else {
        Ok(value)
    }
}

unsafe fn ns_string(value: &str) -> Result<ObjcId> {
    type SendString = unsafe extern "C" fn(ObjcId, ObjcSel, *const c_char) -> ObjcId;
    let value = CString::new(value).map_err(|_| anyhow!("invalid string for ServiceManagement"))?;
    let send: SendString = std::mem::transmute(objc_msgSend as *const ());
    let string = send(
        class("NSString")?,
        selector("stringWithUTF8String:"),
        value.as_ptr(),
    );
    if string.is_null() {
        Err(anyhow!("failed to create ServiceManagement string"))
    } else {
        Ok(string)
    }
}

unsafe fn daemon_service() -> Result<ObjcId> {
    type SendObject = unsafe extern "C" fn(ObjcId, ObjcSel, ObjcId) -> ObjcId;
    let send: SendObject = std::mem::transmute(objc_msgSend as *const ());
    let service = send(
        class("SMAppService")?,
        selector("daemonServiceWithPlistName:"),
        ns_string(DAEMON_PLIST_NAME)?,
    );
    if service.is_null() {
        Err(anyhow!(
            "failed to create macOS LaunchDaemon service object"
        ))
    } else {
        Ok(service)
    }
}

unsafe fn main_app_service() -> Result<ObjcId> {
    type SendObject = unsafe extern "C" fn(ObjcId, ObjcSel) -> ObjcId;
    let send: SendObject = std::mem::transmute(objc_msgSend as *const ());
    let service = send(class("SMAppService")?, selector("mainAppService"));
    if service.is_null() {
        Err(anyhow!("failed to create macOS Login Item service object"))
    } else {
        Ok(service)
    }
}

unsafe fn status_for(service: ObjcId) -> RegistrationStatus {
    type SendStatus = unsafe extern "C" fn(ObjcId, ObjcSel) -> isize;
    let send: SendStatus = std::mem::transmute(objc_msgSend as *const ());
    RegistrationStatus::from_raw(send(service, selector("status")))
}

unsafe fn error_description(error: ObjcId) -> String {
    if error.is_null() {
        return "unknown ServiceManagement error".to_string();
    }
    type SendObject = unsafe extern "C" fn(ObjcId, ObjcSel) -> ObjcId;
    type SendCString = unsafe extern "C" fn(ObjcId, ObjcSel) -> *const c_char;
    let object: SendObject = std::mem::transmute(objc_msgSend as *const ());
    let c_string: SendCString = std::mem::transmute(objc_msgSend as *const ());
    let description = object(error, selector("localizedDescription"));
    let value = c_string(description, selector("UTF8String"));
    if value.is_null() {
        "unknown ServiceManagement error".to_string()
    } else {
        CStr::from_ptr(value).to_string_lossy().into_owned()
    }
}

unsafe fn set_registered(service: ObjcId, enabled: bool) -> Result<RegistrationStatus> {
    type SendAction = unsafe extern "C" fn(ObjcId, ObjcSel, *mut ObjcId) -> i8;
    let send: SendAction = std::mem::transmute(objc_msgSend as *const ());
    let mut error: ObjcId = std::ptr::null_mut();
    let action = if enabled {
        "registerAndReturnError:"
    } else {
        "unregisterAndReturnError:"
    };
    if send(service, selector(action), &mut error) == 0 {
        return Err(anyhow!("{}", error_description(error)));
    }
    Ok(status_for(service))
}

fn with_pool<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let result = operation();
        objc_autoreleasePoolPop(pool);
        result
    }
}

pub fn daemon_status() -> Result<RegistrationStatus> {
    with_pool(|| unsafe { Ok(status_for(daemon_service()?)) })
}

pub fn set_daemon_registered(enabled: bool) -> Result<RegistrationStatus> {
    with_pool(|| unsafe {
        let service = daemon_service()?;
        let current = status_for(service);
        if (enabled
            && matches!(
                current,
                RegistrationStatus::Enabled | RegistrationStatus::RequiresApproval
            ))
            || (!enabled
                && matches!(
                    current,
                    RegistrationStatus::NotRegistered | RegistrationStatus::NotFound
                ))
        {
            return Ok(current);
        }
        set_registered(service, enabled)
    })
}

pub fn set_main_app_login_item(enabled: bool) -> Result<RegistrationStatus> {
    with_pool(|| unsafe {
        let service = main_app_service()?;
        let current = status_for(service);
        if (enabled
            && matches!(
                current,
                RegistrationStatus::Enabled | RegistrationStatus::RequiresApproval
            ))
            || (!enabled
                && matches!(
                    current,
                    RegistrationStatus::NotRegistered | RegistrationStatus::NotFound
                ))
        {
            return Ok(current);
        }
        set_registered(service, enabled)
    })
}
