use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub struct ProductProcess {
    child: Child,
    output_path: PathBuf,
}

impl ProductProcess {
    pub fn spawn(config_path: &Path, output_path: PathBuf) -> Self {
        let output = File::create(&output_path).expect("create product process output");
        let error = output.try_clone().expect("clone product process output");
        let child = Command::new(env!("CARGO_BIN_EXE_mutsuki-bot"))
            .arg(config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(error))
            .spawn()
            .expect("start production mutsuki-bot binary");
        Self { child, output_path }
    }

    pub fn assert_running(&mut self) {
        if let Some(status) = self.child.try_wait().expect("inspect product process") {
            panic!("product exited early with {status}; {}", self.summary());
        }
    }

    pub async fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("inspect product process") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "product did not exit after shutdown; {}",
                self.summary()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub fn output_bytes(&self) -> Vec<u8> {
        std::fs::read(&self.output_path).expect("read product process output")
    }

    pub fn summary(&self) -> String {
        let bytes = self.output_bytes().len();
        format!("captured_bytes={bytes}")
    }
}

impl Drop for ProductProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
