//! Intercept SSH locally: no real host, credential or network is used.
#![cfg(unix)]

use jev_sentinel::collectors::{macos::collect_macos, nvidia::collect_nvidia};
use jev_sentinel::TargetStatus;
use std::os::unix::fs::PermissionsExt;

struct Sandbox {
    path: std::path::PathBuf,
    old_path: Option<std::ffi::OsString>,
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        match &self.old_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[tokio::test]
async fn ssh_collectors_require_pinned_host_keys_and_preserve_failure() {
    // This integration test has its own process and is the only PATH mutator.
    let path = std::env::temp_dir().join(format!("sentinel-ssh-policy-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let sandbox = Sandbox {
        path,
        old_path: std::env::var_os("PATH"),
    };
    let executable = sandbox.path.join("ssh");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' 'Host key verification failed' \"$@\" >&2\nexit 255\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var("PATH", &sandbox.path);
    let mac = collect_macos(
        "mac-policy-test".into(),
        "never-connect.invalid".into(),
        "fixture".into(),
        None,
        None,
        1,
        vec![],
    )
    .await;
    let gpu = collect_nvidia(
        "gpu-policy-test".into(),
        "never-connect.invalid".into(),
        "fixture".into(),
        None,
        None,
        1,
    )
    .await;
    for telemetry in [mac, gpu] {
        assert_eq!(telemetry.status, TargetStatus::Unreachable);
        let error = telemetry.error_message.unwrap();
        assert!(error.contains("Host key verification failed"));
        assert!(error.contains("StrictHostKeyChecking=yes"));
        assert!(!error.contains("StrictHostKeyChecking=no"));
        assert!(error.contains("BatchMode=yes"));
        assert!(error.contains("fixture@never-connect.invalid"));
    }
}
