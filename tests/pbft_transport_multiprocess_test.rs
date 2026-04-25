#[cfg(test)]
mod multiprocess_transport_tests {
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::OnceLock;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use tokio::process::{Child, Command};
    use tokio::sync::Mutex;
    use tokio::time::{sleep, timeout};

    fn worker_bin() -> &'static str {
        env!("CARGO_BIN_EXE_p2p_worker")
    }

    fn suite_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn unique_temp_file(label: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "zoka_{}_{}_{}.addr",
            label,
            std::process::id(),
            now
        ))
    }

    fn soak_seconds(default_secs: u64) -> u64 {
        std::env::var("ZOKA_SOAK_SECS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(default_secs)
    }

    async fn wait_for_file_content(path: &Path, max_wait: Duration) -> Result<String, String> {
        let start = tokio::time::Instant::now();
        loop {
            if path.exists() {
                let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                let trimmed = content.trim().to_string();
                if !trimmed.is_empty() {
                    return Ok(trimmed);
                }
            }

            if start.elapsed() >= max_wait {
                return Err(format!("timeout waiting for file {}", path.display()));
            }

            sleep(Duration::from_millis(60)).await;
        }
    }

    async fn spawn_listener(
        expect_count: u64,
        run_ms: u64,
        expected_sender_prefix: &str,
    ) -> Result<(Child, PathBuf, String), String> {
        let addr_file = unique_temp_file("listener");

        let child = Command::new(worker_bin())
            .args([
                "--expect-pbft",
                &expect_count.to_string(),
                "--expect-sender-prefix",
                expected_sender_prefix,
                "--run-ms",
                &run_ms.to_string(),
                "--addr-file",
                addr_file.to_str().ok_or("invalid addr file path")?,
                "--sender-id",
                "listener",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to spawn listener: {}", e))?;

        let addr = wait_for_file_content(&addr_file, Duration::from_secs(10)).await?;
        Ok((child, addr_file, addr))
    }

    async fn run_sender(
        dial_addr: &str,
        sender_id: &str,
        send_count: u64,
        sequence_start: u64,
        send_interval_ms: u64,
        run_ms: u64,
    ) -> Result<(), String> {
        let output = Command::new(worker_bin())
            .args([
                "--dial",
                dial_addr,
                "--send-pbft",
                &send_count.to_string(),
                "--sequence-start",
                &sequence_start.to_string(),
                "--send-interval-ms",
                &send_interval_ms.to_string(),
                "--run-ms",
                &run_ms.to_string(),
                "--sender-id",
                sender_id,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("failed to run sender {}: {}", sender_id, e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "sender {} exited with status {:?}\nstdout:\n{}\nstderr:\n{}",
                sender_id,
                output.status.code(),
                stdout,
                stderr
            ))
        }
    }

    async fn run_sender_with_retries(
        dial_addr: &str,
        sender_id: &str,
        send_count: u64,
        sequence_start: u64,
        send_interval_ms: u64,
        run_ms: u64,
        max_attempts: u32,
    ) -> Result<(), String> {
        let mut last_error = String::new();
        for attempt in 1..=max_attempts {
            match run_sender(
                dial_addr,
                sender_id,
                send_count,
                sequence_start,
                send_interval_ms,
                run_ms,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_error = format!("attempt {} failed: {}", attempt, err);
                    sleep(Duration::from_millis(300)).await;
                }
            }
        }
        Err(last_error)
    }

    async fn wait_listener_success(listener: &mut Child, timeout_secs: u64) -> Result<(), String> {
        let status = timeout(Duration::from_secs(timeout_secs), listener.wait())
            .await
            .map_err(|_| "listener wait timeout".to_string())?
            .map_err(|e| format!("listener wait failed: {}", e))?;

        if status.success() {
            Ok(())
        } else {
            Err(format!("listener failed with status {:?}", status.code()))
        }
    }

    async fn cleanup_listener(listener: &mut Child) {
        if let Ok(Some(_)) = listener.try_wait() {
            return;
        }
        let _ = listener.kill().await;
        let _ = listener.wait().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_multiprocess_pbft_propagation() {
        let _guard = suite_lock().lock().await;

        let (mut listener, addr_file, listener_addr) = spawn_listener(1, 60_000, "sender-a")
            .await
            .expect("spawn listener");

        let sender_res =
            run_sender_with_retries(&listener_addr, "sender-a", 2, 1, 150, 25_000, 3).await;
        let listener_res = wait_listener_success(&mut listener, 60).await;

        let _ = std::fs::remove_file(&addr_file);
        if sender_res.is_err() || listener_res.is_err() {
            cleanup_listener(&mut listener).await;
        }

        assert!(
            sender_res.is_ok(),
            "{}",
            sender_res.err().unwrap_or_default()
        );
        assert!(
            listener_res.is_ok(),
            "{}",
            listener_res.err().unwrap_or_default()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_multiprocess_pbft_restart_and_soak() {
        let _guard = suite_lock().lock().await;

        let (mut listener, addr_file, listener_addr) = spawn_listener(3, 90_000, "sender-r")
            .await
            .expect("spawn listener");

        let s1 = run_sender_with_retries(&listener_addr, "sender-r1", 2, 100, 150, 25_000, 3).await;
        sleep(Duration::from_millis(200)).await;
        let s2 = run_sender_with_retries(&listener_addr, "sender-r2", 2, 200, 150, 25_000, 3).await;
        sleep(Duration::from_millis(200)).await;
        let s3 = run_sender_with_retries(&listener_addr, "sender-r3", 2, 300, 150, 25_000, 3).await;

        let listener_res = wait_listener_success(&mut listener, 90).await;

        let _ = std::fs::remove_file(&addr_file);
        if s1.is_err() || s2.is_err() || s3.is_err() || listener_res.is_err() {
            eprintln!(
                "diagnostic restart_and_soak: s1={:?} s2={:?} s3={:?} listener={:?}",
                s1, s2, s3, listener_res
            );
            cleanup_listener(&mut listener).await;
        }

        assert!(s1.is_ok(), "{}", s1.err().unwrap_or_default());
        assert!(s2.is_ok(), "{}", s2.err().unwrap_or_default());
        assert!(s3.is_ok(), "{}", s3.err().unwrap_or_default());
        assert!(
            listener_res.is_ok(),
            "{}",
            listener_res.err().unwrap_or_default()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "long-duration soak test (minutes-scale). Run manually or in nightly CI"]
    async fn test_multiprocess_pbft_long_duration_soak() {
        let _guard = suite_lock().lock().await;

        let total_secs = soak_seconds(120);
        let burst_every_secs = 10u64;
        let messages_per_burst = 3u64;
        let bursts = std::cmp::max(1, total_secs / burst_every_secs);
        let expected_total = bursts * messages_per_burst;

        let listener_run_ms = (total_secs + 45) * 1_000;
        let (mut listener, addr_file, listener_addr) =
            spawn_listener(expected_total, listener_run_ms, "soak-sender-")
                .await
                .expect("spawn listener");

        let soak_start = tokio::time::Instant::now();
        let mut sequence_base = 10_000u64;
        let mut sender_failures = Vec::new();

        for burst in 0..bursts {
            let sender_id = format!("soak-sender-{}", burst);
            let result = run_sender_with_retries(
                &listener_addr,
                &sender_id,
                messages_per_burst,
                sequence_base,
                400,
                35_000,
                3,
            )
            .await;
            if let Err(err) = result {
                sender_failures.push(err);
            }
            sequence_base += 1_000;

            let target_elapsed = Duration::from_secs((burst + 1) * burst_every_secs);
            if soak_start.elapsed() < target_elapsed {
                sleep(target_elapsed - soak_start.elapsed()).await;
            }
        }

        let listener_res = wait_listener_success(&mut listener, total_secs + 60).await;

        let _ = std::fs::remove_file(&addr_file);
        if !sender_failures.is_empty() || listener_res.is_err() {
            cleanup_listener(&mut listener).await;
        }

        assert!(
            sender_failures.is_empty(),
            "sender failures: {}",
            sender_failures.join("\n---\n")
        );
        assert!(
            listener_res.is_ok(),
            "{}",
            listener_res.err().unwrap_or_default()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "restart-chaos soak test (minutes-scale). Run manually or in nightly CI"]
    async fn test_multiprocess_pbft_restart_chaos_soak() {
        let _guard = suite_lock().lock().await;

        let total_secs = soak_seconds(90);
        let cycle_secs = 15u64;
        let cycles = std::cmp::max(3, total_secs / cycle_secs);
        let mut sender_failures = Vec::new();
        let mut listener_failures = Vec::new();

        for cycle in 0..cycles {
            let cycle_start = tokio::time::Instant::now();

            let (mut listener, addr_file, listener_addr) = spawn_listener(2, 35_000, "chaos-c")
                .await
                .expect("spawn listener");

            let s1 = run_sender_with_retries(
                &listener_addr,
                &format!("chaos-c{}-s1", cycle),
                1,
                50_000 + cycle * 100,
                0,
                20_000,
                3,
            )
            .await;
            if let Err(err) = s1 {
                sender_failures.push(err);
            }

            if cycle % 2 == 1 {
                cleanup_listener(&mut listener).await;
                let _ = std::fs::remove_file(&addr_file);

                let (mut restarted_listener, restart_addr_file, restarted_addr) =
                    spawn_listener(1, 35_000, "chaos-c")
                        .await
                        .expect("spawn restarted listener");

                let s2 = run_sender_with_retries(
                    &restarted_addr,
                    &format!("chaos-c{}-s2r", cycle),
                    1,
                    70_000 + cycle * 100,
                    0,
                    20_000,
                    3,
                )
                .await;
                if let Err(err) = s2 {
                    sender_failures.push(err);
                }

                let restart_res = wait_listener_success(&mut restarted_listener, 35).await;
                if let Err(err) = restart_res {
                    listener_failures.push(err);
                    cleanup_listener(&mut restarted_listener).await;
                }
                let _ = std::fs::remove_file(&restart_addr_file);
            } else {
                let s2 = run_sender_with_retries(
                    &listener_addr,
                    &format!("chaos-c{}-s2", cycle),
                    1,
                    60_000 + cycle * 100,
                    0,
                    20_000,
                    3,
                )
                .await;
                if let Err(err) = s2 {
                    sender_failures.push(err);
                }

                let listener_res = wait_listener_success(&mut listener, 35).await;
                if let Err(err) = listener_res {
                    listener_failures.push(err);
                    cleanup_listener(&mut listener).await;
                }
                let _ = std::fs::remove_file(&addr_file);
            }

            let jitter_ms = 200 + ((cycle * 37) % 250);
            let target_elapsed = Duration::from_secs(cycle_secs);
            if cycle_start.elapsed() < target_elapsed {
                sleep(target_elapsed - cycle_start.elapsed()).await;
            } else {
                sleep(Duration::from_millis(jitter_ms)).await;
            }
        }

        assert!(
            sender_failures.is_empty(),
            "sender failures: {}",
            sender_failures.join("\n---\n")
        );
        assert!(
            listener_failures.is_empty(),
            "listener failures: {}",
            listener_failures.join("\n---\n")
        );
    }
}
