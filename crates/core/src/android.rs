//! Bounded, version-independent JNI interface to the shared owner rules.
use crate::{
    evaluate, validate_policy, worker_transition, Observation, Policy, Resources, WorkerInput,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "camelCase", deny_unknown_fields)]
enum Request {
    Evaluate {
        policy: Policy,
        observation: Observation,
    },
    Validate {
        policy: Policy,
        host: Resources,
    },
    Transition {
        input: WorkerInput,
    },
}

pub fn android_request(input: &str) -> Result<String, String> {
    if input.len() > 1024 * 1024 {
        return Err("Owner rule request exceeds its supported size".into());
    }
    let request: Request = serde_json::from_str(input).map_err(|_| "Invalid owner rule request")?;
    let result = match request {
        Request::Evaluate {
            policy,
            observation,
        } => serde_json::to_string(&evaluate(&policy, &observation)),
        Request::Validate { policy, host } => {
            serde_json::to_string(&validate_policy(&policy, &host).err())
        }
        Request::Transition { input } => serde_json::to_string(&worker_transition(&input)),
    };
    result.map_err(|_| "Owner rule result could not be encoded".into())
}

/// # Safety
/// The JNI shim supplies valid nonoverlapping input and output buffers of the
/// declared lengths. No pointer is retained and no allocation crosses the ABI.
#[no_mangle]
pub unsafe extern "C" fn nodeharbor_rules(
    input: *const u8,
    length: usize,
    output: *mut u8,
    capacity: usize,
) -> isize {
    if input.is_null() || output.is_null() || length > 1024 * 1024 || capacity > 1024 * 1024 {
        return -1;
    }
    let result = std::panic::catch_unwind(|| {
        let bytes = unsafe { std::slice::from_raw_parts(input, length) };
        let text = std::str::from_utf8(bytes).map_err(|_| ())?;
        android_request(text).map_err(|_| ())
    });
    match result {
        Ok(Ok(text)) if text.len() <= capacity => {
            unsafe {
                std::ptr::copy_nonoverlapping(text.as_ptr(), output, text.len());
            }
            text.len() as isize
        }
        _ => -1,
    }
}
