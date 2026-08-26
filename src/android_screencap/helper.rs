pub const HELPER_REMOTE_DIR: &str = "/data/local/tmp/node-forge";
pub const HELPER_REMOTE_NAME: &str = "node-forge-hdr-screencap";
pub const TARGET_FINGERPRINT: &str =
    "nubia/Nerida/Nerida:16/BQ2A.251016.001-BP2A.250605.031.A3/b1577:userdebug/test-keys";
pub const TARGET_ABI: &str = "arm64-v8a";
pub const TARGET_SDK: &str = "36";

#[cfg(test)]
const TEST_HELPER_BYTES: &[u8] = b"node-forge-test-hdr-helper\n";

#[cfg(all(
    not(test),
    feature = "native",
    not(target_arch = "wasm32"),
    android_hdr_helper_prebuilt
))]
fn shipped_helper_bytes() -> Option<&'static [u8]> {
    Some(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tools/android-hdr-screencap/prebuilt/arm64-v8a/node-forge-hdr-screencap"
    )))
}

#[cfg(all(
    not(test),
    not(all(
        feature = "native",
        not(target_arch = "wasm32"),
        android_hdr_helper_prebuilt
    ))
))]
fn shipped_helper_bytes() -> Option<&'static [u8]> {
    None
}

pub fn embedded_helper_bytes() -> Option<&'static [u8]> {
    #[cfg(test)]
    {
        Some(TEST_HELPER_BYTES)
    }
    #[cfg(not(test))]
    {
        shipped_helper_bytes()
    }
}

pub fn log_helper_capability_once() {
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        if embedded_helper_bytes().is_some() {
            eprintln!(
                "[android-screencap] authoritative HDR helper embedded for fingerprint {TARGET_FINGERPRINT}"
            );
        } else {
            eprintln!(
                "[android-screencap] authoritative HDR helper unavailable: missing fingerprint-tested prebuilt; using stock PNG/gainmap fallback"
            );
        }
    });
}

pub fn helper_remote_path() -> String {
    format!("{HELPER_REMOTE_DIR}/{HELPER_REMOTE_NAME}")
}

pub fn helper_sha256_hex() -> Option<String> {
    embedded_helper_bytes().map(sha256_hex)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn pid_path(request_id: u64) -> String {
    format!("{HELPER_REMOTE_DIR}/capture-{request_id}.pid")
}
