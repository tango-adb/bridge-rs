use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{OnceLock, RwLock},
};

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use ed25519_dalek::{Signature, VerifyingKey};
use rand::Rng;

/// Optional Ed25519 public key embedded at build time (standard base64-encoded 32-byte key).
/// Set the `TANGO_BRIDGE_PUBLIC_KEY` environment variable during `cargo build`.
const PUBLIC_KEY_B64: Option<&str> = option_env!("TANGO_BRIDGE_PUBLIC_KEY");

static PIN: OnceLock<RwLock<String>> = OnceLock::new();

fn home_dir() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(
            std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOMEPATH"))
                .unwrap_or_default(),
        )
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(std::env::var("HOME").unwrap_or_default())
    }
}

fn pin_file_path() -> PathBuf {
    home_dir().join(".android").join("tango-bridge.pin")
}

fn generate_pin() -> String {
    let n: u32 = rand::rng().random_range(0..1_000_000);
    format!("{:06}", n)
}

/// Load the PIN from the file, or generate and save a new one.
/// Must be called once at startup before any other auth functions.
pub fn init() -> std::io::Result<()> {
    let path = pin_file_path();
    let pin = match std::fs::read_to_string(&path) {
        Ok(content) => {
            let s = content.trim().to_string();
            if s.is_empty() {
                let new_pin = generate_pin();
                std::fs::write(&path, &new_pin)?;
                new_pin
            } else {
                s
            }
        }
        Err(_) => {
            let new_pin = generate_pin();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, &new_pin)?;
            new_pin
        }
    };
    PIN.get_or_init(|| RwLock::new(pin));
    Ok(())
}

/// Return the current PIN code.
pub fn get_pin() -> String {
    PIN.get()
        .expect("auth::init() must be called before get_pin()")
        .read()
        .unwrap()
        .clone()
}

/// Generate a new PIN, persist it to the file, and return it.
pub fn regenerate_pin() -> String {
    let pin = generate_pin();
    let path = pin_file_path();
    let _ = std::fs::write(&path, &pin);
    if let Some(lock) = PIN.get() {
        *lock.write().unwrap() = pin.clone();
    }
    pin
}

/// Produce the UTC date strings for today and the adjacent days (±1) to tolerate
/// clock skew between server and client.
fn candidate_dates() -> [String; 3] {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    [secs - 86400, secs, secs + 86400].map(unix_secs_to_date)
}

/// Convert a Unix timestamp (seconds) to a `YYYY-MM-DD` UTC date string.
/// Uses Howard Hinnant's civil-from-days algorithm.
fn unix_secs_to_date(secs: i64) -> String {
    let days = if secs >= 0 {
        secs / 86400
    } else {
        (secs - 86399) / 86400
    };
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Verify a public-key token: a URL-safe or standard base64-encoded Ed25519 signature
/// of the current UTC date string (`YYYY-MM-DD`), signed with the client's private key
/// corresponding to the public key embedded at build time.
fn verify_token(token: &str) -> bool {
    let Some(pk_b64) = PUBLIC_KEY_B64 else {
        return false;
    };

    let Ok(pk_bytes) = STANDARD.decode(pk_b64) else {
        return false;
    };
    let Ok(pk_array): Result<[u8; 32], _> = pk_bytes.try_into() else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&pk_array) else {
        return false;
    };

    // Accept URL-safe (no-padding) or standard base64 for the token value in the URL.
    let Ok(sig_bytes) = URL_SAFE_NO_PAD
        .decode(token)
        .or_else(|_| STANDARD.decode(token))
    else {
        return false;
    };
    let Ok(sig_array): Result<[u8; 64], _> = sig_bytes.try_into() else {
        return false;
    };
    let signature = Signature::from_bytes(&sig_array);

    candidate_dates()
        .iter()
        .any(|date| verifying_key.verify_strict(date.as_bytes(), &signature).is_ok())
}

/// Verify a PIN value against the stored PIN.
fn verify_pin(pin: &str) -> bool {
    PIN.get()
        .map(|lock| *lock.read().unwrap() == pin)
        .unwrap_or(false)
}

/// Return `true` if the query parameters contain a valid `token` (public-key auth)
/// or a valid `pin` (PIN auth).
pub fn is_authenticated(params: &HashMap<String, String>) -> bool {
    if let Some(token) = params.get("token") {
        if verify_token(token) {
            return true;
        }
    }
    if let Some(pin) = params.get("pin") {
        if verify_pin(pin) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unix_secs_to_date() {
        // 2024-01-15 00:00:00 UTC
        assert_eq!(unix_secs_to_date(1705276800), "2024-01-15");
        // 2000-01-01 00:00:00 UTC
        assert_eq!(unix_secs_to_date(946684800), "2000-01-01");
        // 1970-01-01 00:00:00 UTC
        assert_eq!(unix_secs_to_date(0), "1970-01-01");
    }
}
