use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub success: bool,
    pub timed_out: bool,
    pub truncated: bool,
}

pub const DEFAULT_MAX_STDOUT_BYTES: usize = 64 * 1024; // 64 KiB
pub const DEFAULT_MAX_STDERR_BYTES: usize = 16 * 1024; // 16 KiB

pub async fn run_bounded_process(
    program: &str,
    args: &[String],
    timeout_duration: Duration,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
) -> Result<ProcessOutput, std::io::Error> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn()?;
    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");

    let stdout_future = async move {
        let mut buf = Vec::new();
        let mut truncated = false;
        let mut chunk = [0u8; 4096];
        loop {
            let n = stdout_pipe.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            if buf.len() + n > max_stdout_bytes {
                let remaining = max_stdout_bytes.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..remaining]);
                truncated = true;
                while let Ok(m) = stdout_pipe.read(&mut chunk).await {
                    if m == 0 {
                        break;
                    }
                }
                break;
            } else {
                buf.extend_from_slice(&chunk[..n]);
            }
        }
        Ok::<_, std::io::Error>((buf, truncated))
    };

    let stderr_future = async move {
        let mut buf = Vec::new();
        let mut truncated = false;
        let mut chunk = [0u8; 4096];
        loop {
            let n = stderr_pipe.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            if buf.len() + n > max_stderr_bytes {
                let remaining = max_stderr_bytes.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..remaining]);
                truncated = true;
                while let Ok(m) = stderr_pipe.read(&mut chunk).await {
                    if m == 0 {
                        break;
                    }
                }
                break;
            } else {
                buf.extend_from_slice(&chunk[..n]);
            }
        }
        Ok::<_, std::io::Error>((buf, truncated))
    };

    let execution_future = async {
        let (stdout_res, stderr_res, status_res) =
            tokio::join!(stdout_future, stderr_future, child.wait());

        let (stdout_bytes, stdout_trunc) = stdout_res?;
        let (stderr_bytes, stderr_trunc) = stderr_res?;
        let status = status_res?;

        Ok::<_, std::io::Error>(ProcessOutput {
            stdout: String::from_utf8_lossy(&stdout_bytes).trim().to_string(),
            stderr: String::from_utf8_lossy(&stderr_bytes).trim().to_string(),
            exit_code: status.code().unwrap_or(-1),
            success: status.success(),
            timed_out: false,
            truncated: stdout_trunc || stderr_trunc,
        })
    };

    match timeout(timeout_duration, execution_future).await {
        Ok(result) => result,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;

            Ok(ProcessOutput {
                stdout: String::new(),
                stderr: format!("Process timed out after {:?}", timeout_duration),
                exit_code: -1,
                success: false,
                timed_out: true,
                truncated: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_bounded_process_success() {
        let args = vec!["hello world".to_string()];
        let out = run_bounded_process(
            "echo",
            &args,
            Duration::from_secs(2),
            DEFAULT_MAX_STDOUT_BYTES,
            DEFAULT_MAX_STDERR_BYTES,
        )
        .await
        .expect("Command failed to execute");

        assert!(out.success);
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, "hello world");
        assert!(!out.timed_out);
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn test_run_bounded_process_timeout() {
        let args = vec!["5".to_string()];
        let out = run_bounded_process(
            "sleep",
            &args,
            Duration::from_millis(50),
            DEFAULT_MAX_STDOUT_BYTES,
            DEFAULT_MAX_STDERR_BYTES,
        )
        .await
        .expect("Command failed");

        assert!(!out.success);
        assert!(out.timed_out);
        assert_eq!(out.exit_code, -1);
    }

    #[tokio::test]
    async fn test_run_bounded_process_truncation() {
        let long_str = "A".repeat(200);
        let args = vec![long_str];
        let out = run_bounded_process(
            "echo",
            &args,
            Duration::from_secs(2),
            32, // max 32 bytes
            DEFAULT_MAX_STDERR_BYTES,
        )
        .await
        .expect("Command failed");

        assert!(out.success);
        assert!(out.truncated);
        assert!(out.stdout.len() <= 32);
    }
}
